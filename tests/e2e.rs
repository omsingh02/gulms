//! End-to-end tests: run the real binary against a fake Moodle server.
//!
//! Nothing here touches the network beyond loopback or the user's real config:
//! every run gets private `GULMS_CONFIG_DIR` / `GULMS_CACHE_DIR` directories.

use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

const PASSWORD: &str = "correct horse";
const TOKEN: &str = "TOK123";
const FILE_BODY: &[u8] = b"%PDF-1.4 fake lecture";
/// A real deck (generated with python-pptx) served for every `.pptx` the fake portal lists.
const DECK: &[u8] = include_bytes!("fixtures/rich.pptx");

// ---------------------------------------------------------------- fake Moodle

struct Request {
    method: String,
    path: String,
    form: HashMap<String, String>,
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("00");
                out.push(u8::from_str_radix(hex, 16).unwrap_or(b'?'));
                i += 2;
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn parse_form(body: &str) -> HashMap<String, String> {
    body.split('&')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (percent_decode(k), percent_decode(v)))
        .collect()
}

fn read_request(stream: &mut TcpStream) -> Option<Request> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line).ok()?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next()?.to_string();
    let path = parts.next()?.to_string();

    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).ok()?;
        if line == "\r\n" || line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            content_length = value.trim().parse().ok()?;
        }
    }

    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body).ok()?;
    Some(Request {
        method,
        path,
        form: parse_form(&String::from_utf8_lossy(&body)),
    })
}

fn respond(stream: &mut TcpStream, status: &str, content_type: &str, body: &[u8]) {
    let head = format!(
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        status,
        content_type,
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
}

fn json(stream: &mut TcpStream, value: serde_json::Value) {
    respond(
        stream,
        "200 OK",
        "application/json",
        value.to_string().as_bytes(),
    );
}

/// Misbehaviour switches for the fake portal.
#[derive(Default)]
struct Flaky {
    /// Drop the first request for this course's contents, then behave.
    drop_once_for_course: Option<&'static str>,
    /// Drop every request for this course's contents.
    drop_always_for_course: Option<&'static str>,
    dropped_once: AtomicBool,
}

fn handle(mut stream: TcpStream, base: &str, flaky: &Flaky) {
    let Some(req) = read_request(&mut stream) else {
        return;
    };

    if req.form.get("wsfunction").map(String::as_str) == Some("core_course_get_contents") {
        let course = req.form.get("courseid").map(String::as_str);
        let drop_always = course.is_some() && course == flaky.drop_always_for_course;
        let drop_once = course.is_some()
            && course == flaky.drop_once_for_course
            && !flaky.dropped_once.swap(true, Ordering::SeqCst);
        if drop_always || drop_once {
            return; // close the connection without answering
        }
    }

    if req.method == "POST" && req.path == "/login/token.php" {
        if req.form.get("password").map(String::as_str) == Some(PASSWORD)
            && req.form.get("username").map(String::as_str) == Some("alice")
        {
            return json(&mut stream, serde_json::json!({ "token": TOKEN }));
        }
        return json(
            &mut stream,
            serde_json::json!({ "error": "Invalid login, please try again", "errorcode": "invalidlogin" }),
        );
    }

    if req.method == "POST" && req.path == "/webservice/rest/server.php" {
        if req.form.get("wstoken").map(String::as_str) != Some(TOKEN) {
            return json(
                &mut stream,
                serde_json::json!({
                    "exception": "moodle_exception",
                    "errorcode": "invalidtoken",
                    "message": "Invalid token - token not found"
                }),
            );
        }
        return match req.form.get("wsfunction").map(String::as_str) {
            Some("core_webservice_get_site_info") => json(
                &mut stream,
                serde_json::json!({ "userid": 7, "username": "alice", "fullname": "Alice Example" }),
            ),
            Some("core_enrol_get_users_courses") => json(
                &mut stream,
                serde_json::json!([
                    { "id": 1, "fullname": "Computer Organization and Architecture", "shortname": "COA", "timemodified": 100 },
                    { "id": 2, "fullname": "Data Structures", "shortname": "DS", "timemodified": 100 }
                ]),
            ),
            Some("core_course_get_contents") => {
                let course = req.form.get("courseid").cloned().unwrap_or_default();
                let file = |name: &str, size: usize| {
                    serde_json::json!({
                        "filename": name,
                        "filesize": size,
                        "fileurl": format!("{}/pluginfile.php/{}/{}", base, course, name.replace(' ', "%20")),
                        "timecreated": 1_700_000_000u64,
                        "timemodified": 1_700_000_000u64
                    })
                };
                let contents = if course == "1" {
                    vec![
                        file("Lec 1 - Registers.pdf", FILE_BODY.len()),
                        file("DBMS_L3.pptx", DECK.len()),
                        file("DBMS_L3_copy.pptx", DECK.len()),
                    ]
                } else {
                    vec![file("Trees.pdf", FILE_BODY.len())]
                };
                json(
                    &mut stream,
                    serde_json::json!([{
                        "id": 10, "name": "Unit 1",
                        "modules": [{
                            "id": 100, "name": "Lecture", "modname": "resource",
                            "contents": contents
                        }]
                    }]),
                )
            }
            _ => json(
                &mut stream,
                serde_json::json!({ "exception": "x", "message": "unknown function" }),
            ),
        };
    }

    if req.method == "GET" && req.path.starts_with("/pluginfile.php/") {
        if req.path.contains(&format!("token={}", TOKEN)) {
            if req.path.contains(".pptx") {
                return respond(
                    &mut stream,
                    "200 OK",
                    "application/vnd.openxmlformats-officedocument.presentationml.presentation",
                    DECK,
                );
            }
            return respond(&mut stream, "200 OK", "application/pdf", FILE_BODY);
        }
        return respond(&mut stream, "403 Forbidden", "text/plain", b"no token");
    }

    respond(&mut stream, "404 Not Found", "text/plain", b"not found");
}

fn start_server(flaky: Flaky) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let served_base = base.clone();
    let flaky = Arc::new(flaky);
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let (base, flaky) = (served_base.clone(), Arc::clone(&flaky));
            thread::spawn(move || handle(stream, &base, &flaky));
        }
    });
    base
}

// ------------------------------------------------------------------ harness

struct Sandbox {
    root: PathBuf,
    base: String,
}

impl Sandbox {
    fn new(name: &str) -> Self {
        Self::with_flaky(name, Flaky::default())
    }

    fn with_flaky(name: &str, flaky: Flaky) -> Self {
        let root = std::env::temp_dir().join(format!("gulms-e2e-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        Sandbox {
            root,
            base: start_server(flaky),
        }
    }

    fn config_dir(&self) -> PathBuf {
        self.root.join("config")
    }

    fn config_file(&self) -> PathBuf {
        self.config_dir().join("config.json")
    }

    fn run(&self, args: &[&str], stdin: Option<&str>) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_gulms"))
            .args(args)
            .env("GULMS_CONFIG_DIR", self.config_dir())
            .env("GULMS_CACHE_DIR", self.root.join("cache"))
            .env("HOME", &self.root)
            .env("NO_COLOR", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        if let Some(input) = stdin {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(input.as_bytes())
                .unwrap();
        } else {
            drop(child.stdin.take());
        }
        child.wait_with_output().unwrap()
    }

    fn login(&self) -> Output {
        self.run(
            &[
                "login",
                "--url",
                &self.base,
                "--username",
                "alice",
                "--password-stdin",
            ],
            Some(&format!("{}\n", PASSWORD)),
        )
    }

    /// Point downloads at the sandbox, preserving the rest of the saved config.
    /// Sign in, sync and point downloads at the sandbox. Returns the downloads folder.
    fn ready(&self) -> PathBuf {
        assert!(self.login().status.success());
        let downloads = self.set_download_dir();
        let sync = self.run(&["sync"], None);
        assert!(sync.status.success(), "sync failed: {}", stderr(&sync));
        downloads
    }

    /// A config with only a download folder (no login), so `doctor` has somewhere to write.
    fn set_download_dir_unsigned(&self) {
        fs::create_dir_all(self.config_dir()).unwrap();
        let dir = self.root.join("downloads");
        fs::write(
            self.config_file(),
            serde_json::json!({ "base_url": self.base, "download_dir": dir }).to_string(),
        )
        .unwrap();
    }

    fn set_download_dir(&self) -> PathBuf {
        let dir = self.root.join("downloads");
        let mut cfg: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(self.config_file()).unwrap()).unwrap();
        cfg["download_dir"] = serde_json::json!(dir.display().to_string());
        fs::write(self.config_file(), cfg.to_string()).unwrap();
        dir
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn dir_contains(dir: &Path, needle: &str) -> bool {
    fn walk(dir: &Path, needle: &str) -> bool {
        let Ok(entries) = fs::read_dir(dir) else {
            return false;
        };
        entries.flatten().any(|e| {
            let p = e.path();
            if p.is_dir() {
                walk(&p, needle)
            } else {
                fs::read_to_string(&p).is_ok_and(|c| c.contains(needle))
            }
        })
    }
    walk(dir, needle)
}

// -------------------------------------------------------------------- tests

#[test]
fn login_saves_token_privately_and_never_stores_the_password() {
    let sb = Sandbox::new("login");
    let out = sb.login();

    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("Signed in as Alice Example"),
        "{}",
        stdout(&out)
    );

    let cfg = fs::read_to_string(sb.config_file()).unwrap();
    assert!(cfg.contains(TOKEN));
    assert!(cfg.contains("\"user_id\": 7"));
    assert!(
        !dir_contains(&sb.root, PASSWORD),
        "password must never be written to disk"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(sb.config_file()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "config holds a credential");
    }
}

#[test]
fn wrong_password_fails_cleanly_and_saves_nothing() {
    let sb = Sandbox::new("badpass");
    let out = sb.run(
        &[
            "login",
            "--url",
            &sb.base,
            "--username",
            "alice",
            "--password-stdin",
        ],
        Some("nope\n"),
    );

    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("Invalid login"), "{}", stderr(&out));
    assert!(!sb.config_file().exists());
}

#[test]
fn sync_then_browse_search_and_download() {
    let sb = Sandbox::new("flow");
    assert!(sb.login().status.success());
    let downloads = sb.set_download_dir();

    let sync = sb.run(&["sync"], None);
    assert!(sync.status.success(), "stderr: {}", stderr(&sync));
    assert!(
        stdout(&sync).contains("Checked: 2 courses"),
        "{}",
        stdout(&sync)
    );

    let courses = stdout(&sb.run(&["courses"], None));
    assert!(
        courses.contains("Computer Organization and Architecture"),
        "{courses}"
    );
    assert!(courses.contains("Data Structures"), "{courses}");

    let search = stdout(&sb.run(&["search", "registers"], None));
    assert!(search.contains("Lec 1 - Registers.pdf"), "{search}");

    let dl = sb.run(&["download", "registers"], None);
    assert!(dl.status.success(), "stderr: {}", stderr(&dl));
    assert!(
        dir_contains(&downloads, "%PDF-1.4 fake lecture"),
        "file should be downloaded under {downloads:?}"
    );
    assert!(
        !fs::read_dir(downloads.join("COA").join("Lecture Notes & Documents"))
            .unwrap()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().ends_with(".part")),
        "no .part files left behind"
    );
}

#[test]
fn expired_token_tells_the_user_to_sign_in_again() {
    let sb = Sandbox::new("expired");
    assert!(sb.login().status.success());

    let mut cfg: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(sb.config_file()).unwrap()).unwrap();
    cfg["token"] = serde_json::json!("REVOKED");
    fs::write(sb.config_file(), cfg.to_string()).unwrap();

    let out = sb.run(&["sync"], None);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("gulms login"), "{}", stderr(&out));
}

#[test]
fn sync_without_signing_in_points_at_login() {
    let sb = Sandbox::new("anon");
    let out = sb.run(&["sync"], None);

    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("gulms login"), "{}", stderr(&out));
}

#[test]
fn logout_removes_the_token() {
    let sb = Sandbox::new("logout");
    assert!(sb.login().status.success());

    let out = sb.run(&["logout"], None);
    assert!(out.status.success(), "stderr: {}", stderr(&out));

    let cfg = fs::read_to_string(sb.config_file()).unwrap();
    assert!(!cfg.contains(TOKEN), "token should be gone: {cfg}");
}

#[test]
fn switching_accounts_drops_the_previous_users_course_cache() {
    let sb = Sandbox::new("switch");
    assert!(sb.login().status.success());
    assert!(sb.run(&["sync"], None).status.success());
    assert!(stdout(&sb.run(&["courses"], None)).contains("Data Structures"));

    // Pretend the cached data belongs to someone else, then sign in again.
    let mut cfg: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(sb.config_file()).unwrap()).unwrap();
    cfg["user_id"] = serde_json::json!(999);
    cfg["selected_course_ids"] = serde_json::json!([2]);
    fs::write(sb.config_file(), cfg.to_string()).unwrap();

    assert!(sb.login().status.success());

    let cfg: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(sb.config_file()).unwrap()).unwrap();
    assert_eq!(cfg["selected_course_ids"], serde_json::json!([]));
    assert!(stdout(&sb.run(&["courses"], None)).contains("No courses cached"));
}

#[test]
fn bare_invocation_without_a_terminal_prints_usage() {
    let sb = Sandbox::new("usage");
    let out = sb.run(&[], None);

    assert!(out.status.success());
    assert!(stdout(&out).contains("Usage: gulms"), "{}", stdout(&out));
    assert!(
        !sb.config_file().exists(),
        "must not start the wizard without a terminal"
    );
}

fn browser_available() -> bool {
    let candidates = [
        "chromium",
        "chromium-browser",
        "google-chrome",
        "google-chrome-stable",
        "brave",
        "microsoft-edge",
        "msedge",
    ];
    std::env::var_os("CHROME_BIN").is_some()
        || std::env::var_os("PATH").is_some_and(|paths| {
            std::env::split_paths(&paths)
                .any(|dir| candidates.iter().any(|c| dir.join(c).is_file()))
        })
}

#[test]
fn analyze_numbers_and_titles_the_lectures() {
    let sb = Sandbox::new("analyze");
    sb.ready();

    // Before analysis the decks are unnumbered, and in a script we only hint.
    let before = sb.run(&["slides", "COA"], None);
    assert!(before.status.success(), "{}", stderr(&before));
    assert!(
        stderr(&before).contains("gulms analyze"),
        "{}",
        stderr(&before)
    );
    assert!(stdout(&before).contains("[Extra ]"), "{}", stdout(&before));

    let analyze = sb.run(&["analyze"], None);
    assert!(analyze.status.success(), "stderr: {}", stderr(&analyze));
    assert!(
        stdout(&analyze).contains("Analysed 2 slide files"),
        "{}",
        stdout(&analyze)
    );

    // Both uploads are the same lecture: one canonical entry with the real title.
    let after = stdout(&sb.run(&["slides", "COA"], None));
    assert!(after.contains("[Lec 03] Normal Forms"), "{after}");
    assert!(after.contains("1 canonical"), "{after}");
    assert!(!after.contains("[Extra ]"), "{after}");

    let all = stdout(&sb.run(&["slides", "COA", "--all"], None));
    assert!(
        all.contains("Lec-03 - Normal Forms.pptx") && all.contains("4"),
        "{all}"
    );

    // Running it again has nothing to do.
    let again = sb.run(&["analyze"], None);
    assert!(
        stdout(&again).contains("already analysed"),
        "{}",
        stdout(&again)
    );

    let meta = fs::read_to_string(sb.root.join("cache").join("ppt_meta.json")).unwrap();
    assert!(meta.contains("\"lecture_num\": 3"), "{meta}");
}

#[test]
fn export_writes_markdown_study_notes_with_figures_and_diagrams() {
    let sb = Sandbox::new("export");
    let downloads = sb.ready();

    let out = sb.run(&["export", "COA", "3", "--no-pdf", "--no-ocr"], None);
    assert!(
        out.status.success(),
        "stderr: {}\nstdout: {}",
        stderr(&out),
        stdout(&out)
    );

    let notes = downloads.join("COA").join("Study Notes");
    let md = fs::read_to_string(notes.join("Lec-03 - Normal Forms.md")).unwrap();
    assert!(md.starts_with("# Lecture 3: Normal Forms"), "{md}");
    assert!(md.contains("> Source: `DBMS_L3.pptx` (4 slides)"), "{md}");
    assert!(md.contains("## Slide 1: Normal Forms"));
    assert!(
        md.contains("- Second line one second line two"),
        "soft break should become a space"
    );
    assert!(md.contains("| Key | Meaning | Example |"));
    assert!(
        md.contains("flowchart ") && md.contains("-- yes -->"),
        "{md}"
    );
    assert!(
        !md.contains("Two plain boxes\n```mermaid"),
        "unconnected boxes are not a diagram"
    );

    // The real figure was saved next to the notes and linked relatively without spaces.
    let figure = notes
        .join("figures")
        .join("lec-03-normal-forms")
        .join("fig_s02_01.png");
    assert!(figure.is_file(), "missing {figure:?}");
    assert!(
        md.contains("](figures/lec-03-normal-forms/fig_s02_01.png)"),
        "{md}"
    );
}

#[test]
fn export_unknown_lecture_lists_what_exists() {
    let sb = Sandbox::new("export-missing");
    sb.ready();

    let out = sb.run(&["export", "COA", "99", "--no-pdf", "--no-ocr"], None);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("lecture 99 not found") && stderr(&out).contains("available: 3"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn export_produces_a_pdf_with_an_outline_when_a_browser_exists() {
    if !browser_available() {
        eprintln!("skipped: no Chromium-family browser installed");
        return;
    }
    let sb = Sandbox::new("export-pdf");
    let downloads = sb.ready();

    let out = sb.run(&["export", "COA", "3", "--no-ocr"], None);
    assert!(out.status.success(), "stderr: {}", stderr(&out));

    let pdf = fs::read(
        downloads
            .join("COA")
            .join("Study Notes")
            .join("Lec-03 - Normal Forms.pdf"),
    )
    .unwrap();
    assert!(pdf.starts_with(b"%PDF-") && pdf.len() > 5_000);
    assert!(String::from_utf8_lossy(&pdf).contains("/Outlines"));
}

#[test]
fn download_course_keeps_one_copy_of_each_lecture_under_its_real_name() {
    let sb = Sandbox::new("dlcourse");
    let downloads = sb.ready();

    let out = sb.run(&["download-course", "COA", "--yes"], None);
    assert!(
        out.status.success(),
        "stderr: {}\nstdout: {}",
        stderr(&out),
        stdout(&out)
    );

    let slides = downloads.join("COA").join("Lecture Slides (PPT)");
    let names: Vec<String> = fs::read_dir(&slides)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names,
        vec!["Lec-03 - Normal Forms.pptx"],
        "duplicate uploads collapse to one"
    );
    assert_eq!(
        fs::read(slides.join("Lec-03 - Normal Forms.pptx")).unwrap(),
        DECK
    );
    assert!(
        downloads
            .join("COA")
            .join("Lecture Notes & Documents")
            .join("Lec 1 - Registers.pdf")
            .is_file()
    );

    // A second run finds everything already there.
    let again = sb.run(&["download-course", "COA", "--yes"], None);
    assert!(
        stdout(&again).contains("2 already present"),
        "{}",
        stdout(&again)
    );
}

#[test]
fn download_course_without_yes_refuses_to_guess_in_a_script() {
    let sb = Sandbox::new("dlcourse-prompt");
    sb.ready();

    let out = sb.run(&["download-course", "COA"], None);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("--yes"), "{}", stderr(&out));
}

#[test]
fn view_by_section_and_search_download_to_a_folder() {
    let sb = Sandbox::new("misc");
    sb.ready();

    let view = stdout(&sb.run(&["view", "COA", "--by-section"], None));
    assert!(view.contains("Unit 1 (3 files)"), "{view}");

    let dest = sb.root.join("flat");
    let out = sb.run(
        &[
            "search",
            "registers",
            "--download",
            "--dest",
            dest.to_str().unwrap(),
        ],
        None,
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(
        fs::read(dest.join("Lec 1 - Registers.pdf")).unwrap(),
        FILE_BODY
    );
}

#[test]
fn doctor_flags_a_missing_login_and_exits_nonzero() {
    let sb = Sandbox::new("doctor-anon");
    sb.set_download_dir_unsigned();

    let out = sb.run(&["doctor", "--offline"], None);
    assert_eq!(out.status.code(), Some(1));
    let text = stdout(&out);
    assert!(
        text.contains("Signed in") && text.contains("run `gulms login`"),
        "{text}"
    );
    assert!(text.contains("1 problem to fix"), "{text}");
}

#[test]
fn doctor_confirms_a_healthy_setup() {
    let sb = Sandbox::new("doctor-ok");
    sb.ready();
    assert!(sb.run(&["analyze"], None).status.success());

    let out = sb.run(&["doctor"], None);
    let text = stdout(&out);
    assert!(out.status.success(), "{text}\n{}", stderr(&out));
    assert!(text.contains("as Alice Example"), "{text}");
    assert!(text.contains("is reachable"), "{text}");
    assert!(text.contains("2 cached, last synced today"), "{text}");
    assert!(text.contains("all 2 slide decks analysed"), "{text}");
}

#[test]
fn doctor_reports_an_expired_session() {
    let sb = Sandbox::new("doctor-expired");
    sb.ready();
    let mut cfg: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(sb.config_file()).unwrap()).unwrap();
    cfg["token"] = serde_json::json!("REVOKED");
    fs::write(sb.config_file(), cfg.to_string()).unwrap();

    let out = sb.run(&["doctor"], None);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stdout(&out).contains("your session expired"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn completions_cover_the_subcommands() {
    let sb = Sandbox::new("completions");
    for (shell, marker) in [
        ("bash", "_gulms()"),
        ("zsh", "#compdef gulms"),
        ("fish", "complete -c gulms"),
        ("powershell", "Register-ArgumentCompleter"),
    ] {
        let out = sb.run(&["completions", shell], None);
        assert!(out.status.success(), "{shell}: {}", stderr(&out));
        let text = stdout(&out);
        assert!(
            text.contains(marker),
            "{shell} script should contain {marker}"
        );
        for cmd in ["export", "doctor", "download-course"] {
            assert!(text.contains(cmd), "{shell} script should offer `{cmd}`");
        }
    }
}

#[cfg(unix)]
#[test]
fn a_config_readable_by_others_is_locked_down_on_next_run() {
    use std::os::unix::fs::PermissionsExt;
    let sb = Sandbox::new("perms");
    assert!(sb.login().status.success());
    fs::set_permissions(sb.config_file(), fs::Permissions::from_mode(0o644)).unwrap();

    let out = sb.run(&["whoami"], None);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stderr(&out).contains("restricted"), "{}", stderr(&out));
    assert_eq!(
        fs::metadata(sb.config_file()).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn completions_work_even_when_the_config_is_broken() {
    let sb = Sandbox::new("completions-broken");
    fs::create_dir_all(sb.config_dir()).unwrap();
    fs::write(sb.config_file(), "this is {not json").unwrap();

    let out = sb.run(&["completions", "bash"], None);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("_gulms"));
    // ...while an ordinary command still reports the broken file clearly.
    let other = sb.run(&["courses"], None);
    assert_eq!(other.status.code(), Some(1));
    assert!(
        stderr(&other).contains("Invalid config file"),
        "{}",
        stderr(&other)
    );
}

#[test]
fn a_dropped_connection_is_retried_and_the_sync_still_succeeds() {
    let sb = Sandbox::with_flaky(
        "retry",
        Flaky {
            drop_once_for_course: Some("2"),
            ..Flaky::default()
        },
    );
    assert!(sb.login().status.success());

    let sync = sb.run(&["sync"], None);
    assert!(sync.status.success(), "stderr: {}", stderr(&sync));
    assert!(stdout(&sync).contains("Fetched: 2"), "{}", stdout(&sync));
    assert!(stdout(&sb.run(&["courses"], None)).contains("Data Structures"));
}

#[test]
fn a_course_that_keeps_failing_is_reported_and_fails_the_sync() {
    let sb = Sandbox::with_flaky(
        "partial",
        Flaky {
            drop_always_for_course: Some("2"),
            ..Flaky::default()
        },
    );
    assert!(sb.login().status.success());

    let sync = sb.run(&["sync"], None);
    assert_eq!(
        sync.status.code(),
        Some(1),
        "a partial sync must not look like success"
    );
    let err = stderr(&sync);
    assert!(
        err.contains("1 course could not be fetched") && err.contains("DS"),
        "{err}"
    );

    // The course that worked is still usable.
    let courses = stdout(&sb.run(&["courses"], None));
    assert!(
        courses.contains("Computer Organization and Architecture"),
        "{courses}"
    );
    assert!(!courses.contains("Data Structures"), "{courses}");
}
