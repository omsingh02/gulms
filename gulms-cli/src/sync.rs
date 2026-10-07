use colored::*;
use indicatif::{ProgressBar, ProgressStyle};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::time::Instant;

use crate::categorizer::clean_text;
use crate::models::{RawCourse, RawSection, SyncState};
use crate::store::LmsStore;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SiteInfo {
    pub sitename: Option<String>,
    pub username: Option<String>,
    pub fullname: Option<String>,
    pub userid: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrolledCourseSummary {
    pub id: u64,
    #[serde(default)]
    pub fullname: String,
    pub shortname: Option<String>,
    #[serde(default)]
    pub timemodified: u64,
}

pub struct SyncStats {
    pub checked_count: usize,
    pub updated_courses: Vec<String>,
    pub new_files_count: usize,
    pub elapsed: std::time::Duration,
}

pub struct LmsClient {
    pub base_url: String,
    pub token: Option<String>,
    pub user_id: Option<u64>,
}

impl LmsClient {
    pub fn new(base_url: String, token: Option<String>, user_id: Option<u64>) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            token,
            user_id,
        }
    }

    pub fn call(
        &self,
        wsfunction: &str,
        extra_params: &[(&str, &str)],
    ) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
        let token = self
            .token
            .as_deref()
            .ok_or("No token configured in ~/.config/gulms/config.json")?;

        let url = format!("{}/webservice/rest/server.php", self.base_url);

        let mut form_data = vec![
            ("wstoken", token),
            ("wsfunction", wsfunction),
            ("moodlewsrestformat", "json"),
        ];
        form_data.extend_from_slice(extra_params);

        let mut resp = ureq::post(&url)
            .header("User-Agent", "Mozilla/5.0 (MoodleMobile; Android)")
            .header("Accept", "application/json")
            .send_form(form_data)?;

        let val: serde_json::Value = resp.body_mut().read_json()?;

        if let Some(err) = val.get("exception") {
            let msg = val
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("Unknown Moodle exception");
            return Err(format!("Moodle API Exception ({}): {}", err, msg).into());
        }

        if let Some(err) = val.get("error") {
            let msg = err.as_str().unwrap_or("Unknown error");
            return Err(format!("Moodle API Error: {}", msg).into());
        }

        Ok(val)
    }

    pub fn get_site_info(&self) -> Result<SiteInfo, Box<dyn std::error::Error>> {
        let val = self.call("core_webservice_get_site_info", &[])?;
        let info: SiteInfo = serde_json::from_value(val)?;
        Ok(info)
    }

    pub fn get_user_courses(
        &self,
        user_id: u64,
    ) -> Result<Vec<EnrolledCourseSummary>, Box<dyn std::error::Error>> {
        let uid_str = user_id.to_string();
        let val = self.call("core_enrol_get_users_courses", &[("userid", &uid_str)])?;
        let courses: Vec<EnrolledCourseSummary> = serde_json::from_value(val)?;
        Ok(courses)
    }

    pub fn get_course_contents(
        &self,
        course_id: u64,
    ) -> Result<Vec<RawSection>, Box<dyn std::error::Error>> {
        let cid_str = course_id.to_string();
        let val = self.call("core_course_get_contents", &[("courseid", &cid_str)])?;
        let sections: Vec<RawSection> = serde_json::from_value(val)?;
        Ok(sections)
    }

    pub fn sync(
        &mut self,
        store: &mut LmsStore,
        force: bool,
    ) -> Result<SyncStats, Box<dyn std::error::Error>> {
        let start_time = Instant::now();

        // 1. Verify user_id or fetch it
        let uid = match self.user_id.or(store.config.user_id) {
            Some(id) => id,
            None => {
                let info = self.get_site_info()?;
                let id = info.userid.ok_or("Could not retrieve user ID from Moodle")?;
                self.user_id = Some(id);
                store.config.user_id = Some(id);
                if let Some(fn_str) = info.fullname {
                    store.config.fullname = Some(fn_str);
                }
                if let Some(un_str) = info.username {
                    store.config.username = Some(un_str);
                }
                store.save_config()?;
                id
            }
        };

        println!(
            "{} Fetching enrolled course list from Moodle...",
            "•".cyan().bold()
        );
        let raw_enrolled = self.get_user_courses(uid)?;
        let checked_count = raw_enrolled.len();

        // 2. Load existing cache map
        let mut cache_map: HashMap<String, RawCourse> = if store.courses_cache_path.exists() {
            let data = fs::read_to_string(&store.courses_cache_path).unwrap_or_default();
            serde_json::from_str(&data).unwrap_or_default()
        } else {
            HashMap::new()
        };

        let mut updated_courses = Vec::new();
        let mut new_files_count = 0usize;

        // Progress bar for scanning courses
        let pb = ProgressBar::new(raw_enrolled.len() as u64);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} [{bar:30.cyan/blue}] {pos}/{len} {msg}")
                .unwrap_or_else(|_| ProgressStyle::default_bar())
                .progress_chars("━╸ "),
        );

        for rc in raw_enrolled {
            let cid_str = rc.id.to_string();
            let cname = clean_text(&rc.fullname);
            let cshort = rc.shortname.map(|s| clean_text(&s)).unwrap_or_default();
            let remote_mod = rc.timemodified;

            let cached_entry = cache_map.get(&cid_str);
            let needs_fetch = force
                || cached_entry.is_none()
                || remote_mod > cached_entry.and_then(|c| c.timemodified).unwrap_or(0);

            let label = if !cshort.is_empty() {
                cshort.clone()
            } else {
                cname.clone()
            };
            pb.set_message(format!("{}", label));

            if needs_fetch {
                // Collect existing files for diffing
                let mut existing_keys = HashSet::new();
                if let Some(entry) = cached_entry {
                    for sec in &entry.sections {
                        for m in &sec.modules {
                            for f in &m.contents {
                                if let Some(ref fname) = f.filename {
                                    existing_keys.insert((fname.clone(), f.filesize.unwrap_or(0)));
                                }
                            }
                        }
                    }
                }

                // Fetch new contents
                match self.get_course_contents(rc.id) {
                    Ok(sections) => {
                        for sec in &sections {
                            for m in &sec.modules {
                                for f in &m.contents {
                                    if let Some(ref fname) = f.filename {
                                        let key = (fname.clone(), f.filesize.unwrap_or(0));
                                        if !existing_keys.is_empty() && !existing_keys.contains(&key) {
                                            new_files_count += 1;
                                        }
                                    }
                                }
                            }
                        }

                        cache_map.insert(
                            cid_str,
                            RawCourse {
                                id: rc.id,
                                fullname: Some(cname.clone()),
                                shortname: if !cshort.is_empty() {
                                    Some(cshort)
                                } else {
                                    None
                                },
                                timemodified: Some(remote_mod),
                                sections,
                            },
                        );
                        updated_courses.push(label);
                    }
                    Err(e) => {
                        eprintln!(
                            "  {} Failed to fetch contents for {}: {}",
                            "⚠".yellow().bold(),
                            label,
                            e
                        );
                    }
                }
            } else if let Some(entry) = cache_map.get_mut(&cid_str) {
                entry.fullname = Some(cname);
                entry.shortname = if !cshort.is_empty() {
                    Some(cshort)
                } else {
                    None
                };
            }

            pb.inc(1);
        }

        pb.finish_and_clear();

        // 3. Save updated cache
        if let Some(parent) = store.courses_cache_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let cache_json = serde_json::to_string_pretty(&cache_map)?;
        fs::write(&store.courses_cache_path, cache_json)?;

        // 4. Save sync state
        let now_ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let state = SyncState {
            last_sync: Some(now_ts),
            last_sync_human: Some(chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()),
            modified_courses: updated_courses.clone(),
            new_files_count: Some(new_files_count),
        };
        let state_json = serde_json::to_string_pretty(&state)?;
        fs::write(&store.sync_state_path, state_json)?;

        // 5. Reload courses in store
        let reloaded_store = LmsStore::load()?;
        store.courses = reloaded_store.courses;
        store.sync_state = Some(state);

        Ok(SyncStats {
            checked_count,
            updated_courses,
            new_files_count,
            elapsed: start_time.elapsed(),
        })
    }
}
