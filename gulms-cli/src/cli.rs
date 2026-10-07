use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "gulms",
    author = "faulter",
    version = "0.1.0",
    about = "Blazing fast interactive CLI for Galgotias University LMS"
)]
pub struct Cli {
    #[arg(short, long, help = "Launch interactive inline menu mode")]
    pub interactive: bool,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    #[command(about = "Display student profile, LMS account, and cache status", alias = "info")]
    Whoami,

    #[command(about = "List enrolled courses", alias = "ls")]
    Courses {
        #[arg(short, long, help = "Show all enrolled courses, not just tracked ones")]
        all: bool,
    },

    #[command(about = "Interactively configure which courses to actively track")]
    Select,

    #[command(about = "Synchronize courses & materials from Moodle LMS")]
    Sync {
        #[arg(short, long, help = "Force full re-sync instead of incremental delta sync")]
        force: bool,
    },

    #[command(about = "List or browse deduplicated slides")]
    Slides {
        #[arg(help = "Course acronym (e.g. DBMS, COA), code, name, or index")]
        course: Option<String>,
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
    },

    #[command(about = "Search for materials by filename, topic, or module")]
    Search {
        #[arg(help = "Search query string")]
        query: String,

        #[arg(short, long, help = "Optional course filter")]
        course: Option<String>,
    },

    #[command(about = "View materials uploaded or modified recently", alias = "new")]
    Recent {
        #[arg(short, long, default_value = "7", help = "Number of days back to look")]
        days: u64,

        #[arg(short, long, help = "Search all courses, not just tracked ones")]
        all: bool,
    },

    #[command(about = "Download matching material", alias = "dl")]
    Download {
        #[arg(help = "Search query or filename to download")]
        query: String,

        #[arg(short, long, help = "Course filter")]
        course: Option<String>,
    },

    #[command(about = "Download (if not cached) and open material in viewer (zathura/xdg-open)")]
    Open {
        #[arg(help = "Search query or filename to open")]
        query: String,

        #[arg(short, long, help = "Course filter")]
        course: Option<String>,
    },
}
