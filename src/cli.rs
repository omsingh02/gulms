use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    version,
    about = "Your university LMS, from the terminal",
    long_about = "Your university LMS, from the terminal: sync courses, find lectures, and turn slides\n\
        into study notes. Built for Galgotias University's Moodle portal.\n\n\
        Run without arguments in a terminal to open the interactive menu.",
    after_help = "EXAMPLES:\n  \
        gulms setup                 First-run guided setup (sign in, pick courses)\n  \
        gulms sync                  Fetch the latest courses and materials\n  \
        gulms courses               List your tracked courses\n  \
        gulms slides DBMS           Lectures with real titles and slide counts\n  \
        gulms export DBMS 3 --open  Study notes (Markdown + PDF) for lecture 3\n  \
        gulms search normalization  Find materials across all courses\n  \
        gulms open \"lec 3\" -c COA   Download (if needed) and open a file\n\n\
        Run `gulms doctor` if something isn't working. Set NO_COLOR=1 to disable colors.",
    args_conflicts_with_subcommands = true
)]
pub struct Cli {
    #[arg(
        short,
        long,
        help = "Launch interactive inline menu mode (default without a command)"
    )]
    pub interactive: bool,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    #[command(about = "Guided first-run setup: sign in, pick courses, choose a download folder")]
    Setup,

    #[command(about = "Sign in with your LMS username and password")]
    Login {
        #[arg(short, long, help = "LMS username / admission number")]
        username: Option<String>,

        #[arg(long, help = "LMS portal URL (default: the Galgotias portal)")]
        url: Option<String>,

        #[arg(
            long,
            help = "Read the password from stdin (for scripts); never pass it as an argument"
        )]
        password_stdin: bool,
    },

    #[command(about = "Sign out and remove the saved token")]
    Logout,

    #[command(
        about = "Display student profile, LMS account, and cache status",
        alias = "info"
    )]
    Whoami,

    #[command(about = "Check your setup and say how to fix anything that is wrong")]
    Doctor {
        #[arg(long, help = "Skip the checks that need the network")]
        offline: bool,
    },

    #[command(about = "Print a shell completion script (bash, zsh, fish, powershell, elvish)")]
    Completions {
        #[arg(value_enum, help = "Shell to generate completions for")]
        shell: clap_complete::Shell,
    },

    #[command(about = "List enrolled courses", alias = "ls")]
    Courses {
        #[arg(short, long, help = "Show all enrolled courses, not just tracked ones")]
        all: bool,

        #[arg(short, long, help = "Sync with the portal first")]
        refresh: bool,
    },

    #[command(about = "Interactively configure which courses to actively track")]
    Select,

    #[command(about = "Synchronize courses & materials from Moodle LMS")]
    Sync {
        #[arg(
            short,
            long,
            help = "Force full re-sync instead of incremental delta sync"
        )]
        force: bool,
    },

    #[command(
        about = "List lectures with their titles and slide counts (opens a picker without a course)"
    )]
    Slides {
        #[arg(help = "Course acronym (e.g. DBMS, COA), code, name, or index")]
        course: Option<String>,

        #[arg(
            short,
            long,
            help = "Show every upload, not just the best version of each lecture"
        )]
        all: bool,

        #[arg(short, long, help = "Show each lecture's agenda")]
        outline: bool,

        #[arg(short, long, help = "Sync with the portal first")]
        refresh: bool,
    },

    #[command(about = "Read slide decks to number and title lectures (downloads them once)")]
    Analyze {
        #[arg(help = "Course to analyse (default: all tracked courses)")]
        course: Option<String>,
    },

    #[command(
        about = "Turn lecture slides into study notes: Markdown plus a PDF with diagrams and tables"
    )]
    Export {
        #[arg(help = "Course acronym, code, name, or index")]
        course: String,

        #[arg(help = "Lecture number (default: every lecture)")]
        lecture: Option<u32>,

        #[arg(long, help = "Folder for the notes (default: your download folder)")]
        dest: Option<PathBuf>,

        #[arg(long, help = "Write Markdown only, skip the PDF")]
        no_pdf: bool,

        #[arg(long, help = "Do not read text out of images (faster)")]
        no_ocr: bool,

        #[arg(short, long, help = "Open the result when exporting a single lecture")]
        open: bool,
    },

    #[command(about = "List lecture notes and documents")]
    Notes {
        #[arg(help = "Course acronym, code, name, or index")]
        course: Option<String>,
    },

    #[command(about = "View materials categorized for a course")]
    View {
        #[arg(help = "Course acronym, code, name, or index")]
        course: Option<String>,

        #[arg(long, help = "Group by course section instead of by category")]
        by_section: bool,

        #[arg(short, long, help = "Sync with the portal first")]
        refresh: bool,
    },

    #[command(about = "Search for materials by filename, topic, or module")]
    Search {
        #[arg(help = "Search query string")]
        query: String,

        #[arg(short, long, help = "Optional course filter")]
        course: Option<String>,

        #[arg(short, long, help = "Download every match")]
        download: bool,

        #[arg(
            long,
            requires = "download",
            help = "Folder for downloads (default: your download folder)"
        )]
        dest: Option<PathBuf>,
    },

    #[command(about = "View materials uploaded or modified recently", alias = "new")]
    Recent {
        #[arg(
            short,
            long,
            default_value_t = 7,
            value_parser = clap::value_parser!(u64).range(1..),
            help = "Number of days back to look"
        )]
        days: u64,

        #[arg(short, long, help = "Search all courses, not just tracked ones")]
        all: bool,

        #[arg(long, help = "Also download every file listed into this folder")]
        dest: Option<PathBuf>,
    },

    #[command(about = "Download matching material", alias = "dl")]
    Download {
        #[arg(help = "Search query or filename to download")]
        query: String,

        #[arg(short, long, help = "Course filter")]
        course: Option<String>,

        #[arg(long, help = "Download every match instead of choosing one")]
        all: bool,

        #[arg(long, help = "Folder for downloads (default: your download folder)")]
        dest: Option<PathBuf>,
    },

    #[command(
        about = "Download a whole course: one copy of each lecture named Lec-NN - Title, plus everything else"
    )]
    DownloadCourse {
        #[arg(help = "Course acronym, code, name, or index")]
        course: String,

        #[arg(long, help = "Folder for the course (default: your download folder)")]
        dest: Option<PathBuf>,

        #[arg(short, long, help = "Do not ask for confirmation")]
        yes: bool,

        #[arg(short, long, help = "Sync with the portal first")]
        refresh: bool,
    },

    #[command(about = "Download (if not cached) and open material in your default viewer")]
    Open {
        #[arg(help = "Search query or filename to open")]
        query: String,

        #[arg(short, long, help = "Course filter")]
        course: Option<String>,
    },
}
