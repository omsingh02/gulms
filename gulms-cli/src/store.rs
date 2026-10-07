use md5::{Digest, Md5};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use crate::models::{Config, Course, PptMeta, RawCourse, SyncState};

#[derive(Debug, Clone)]
pub struct MaterialResult<'a> {
    pub material: &'a crate::models::Material,
    pub course: &'a Course,
}

pub struct LmsStore {
    pub config: Config,
    pub config_path: PathBuf,
    pub courses_cache_path: PathBuf,
    pub ppt_meta_path: PathBuf,
    pub sync_state_path: PathBuf,
    pub courses: Vec<Course>,
    pub ppt_meta: HashMap<String, PptMeta>,
    pub sync_state: Option<SyncState>,
}

impl LmsStore {
    pub fn default_paths() -> (PathBuf, PathBuf, PathBuf, PathBuf) {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/home/faulter"));
        let config_dir = home.join(".config").join("gulms");
        let cache_dir = home.join(".cache").join("gulms");

        (
            config_dir.join("config.json"),
            cache_dir.join("courses.json"),
            cache_dir.join("ppt_meta.json"),
            cache_dir.join("sync_state.json"),
        )
    }

    pub fn load() -> Result<Self, Box<dyn std::error::Error>> {
        let (config_path, courses_cache_path, ppt_meta_path, sync_state_path) =
            Self::default_paths();

        // 1. Config
        let config: Config = if config_path.exists() {
            let data = fs::read_to_string(&config_path)?;
            serde_json::from_str(&data).unwrap_or_default()
        } else {
            Config::default()
        };

        // 2. Courses Cache
        let mut courses: Vec<Course> = Vec::new();
        if courses_cache_path.exists() {
            let data = fs::read_to_string(&courses_cache_path)?;
            if let Ok(raw_dict) = serde_json::from_str::<HashMap<String, RawCourse>>(&data) {
                let mut parsed: Vec<Course> = raw_dict
                    .into_values()
                    .map(Course::from_raw)
                    .collect();
                // Sort by ID or acronym
                parsed.sort_by(|a, b| a.clean_name.cmp(&b.clean_name));
                courses = parsed;
            }
        }

        // 3. PPT Meta
        let ppt_meta: HashMap<String, PptMeta> = if ppt_meta_path.exists() {
            let data = fs::read_to_string(&ppt_meta_path)?;
            serde_json::from_str(&data).unwrap_or_default()
        } else {
            HashMap::new()
        };

        // 4. Sync State
        let sync_state: Option<SyncState> = if sync_state_path.exists() {
            let data = fs::read_to_string(&sync_state_path)?;
            serde_json::from_str(&data).ok()
        } else {
            None
        };

        Ok(Self {
            config,
            config_path,
            courses_cache_path,
            ppt_meta_path,
            sync_state_path,
            courses,
            ppt_meta,
            sync_state,
        })
    }

    pub fn save_config(&self) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(parent) = self.config_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json_str = serde_json::to_string_pretty(&self.config)?;
        fs::write(&self.config_path, json_str)?;
        Ok(())
    }

    pub fn save_selected_courses(&mut self, ids: Vec<u64>) -> Result<(), Box<dyn std::error::Error>> {
        self.config.selected_course_ids = ids;
        self.save_config()
    }

    pub fn courses(&self, show_all: bool) -> Vec<&Course> {
        if show_all || self.config.selected_course_ids.is_empty() {
            self.courses.iter().collect()
        } else {
            self.courses
                .iter()
                .filter(|c| self.config.selected_course_ids.contains(&c.id))
                .collect()
        }
    }

    pub fn all_courses(&self) -> &[Course] {
        &self.courses
    }

    pub fn get_course(&self, query: &str) -> Option<&Course> {
        let q = query.trim();
        if q.is_empty() {
            return None;
        }

        let pool = self.courses(false);
        let all_pool: Vec<&Course> = self.courses.iter().collect();

        // 1. By index in active pool if numeric
        if let Ok(idx) = q.parse::<usize>() {
            if idx > 0 && idx <= pool.len() {
                return Some(pool[idx - 1]);
            }
            // By raw ID
            if let Ok(id_val) = q.parse::<u64>() {
                if let Some(c) = all_pool.iter().find(|c| c.id == id_val) {
                    return Some(*c);
                }
            }
        }

        let q_lower = q.to_lowercase();
        let q_upper = q.to_uppercase();

        // 2. Exact acronym match
        if let Some(c) = pool.iter().find(|c| c.acronym.to_uppercase() == q_upper) {
            return Some(*c);
        }
        if let Some(c) = all_pool.iter().find(|c| c.acronym.to_uppercase() == q_upper) {
            return Some(*c);
        }

        // 3. Substring in shortname or clean_name
        if let Some(c) = pool.iter().find(|c| {
            c.shortname.to_lowercase().contains(&q_lower)
                || c.clean_name.to_lowercase().contains(&q_lower)
        }) {
            return Some(*c);
        }

        if let Some(c) = all_pool.iter().find(|c| {
            c.shortname.to_lowercase().contains(&q_lower)
                || c.clean_name.to_lowercase().contains(&q_lower)
        }) {
            return Some(*c);
        }

        None
    }

    pub fn search_materials(
        &self,
        query: &str,
        course_filter: Option<&str>,
        show_all_courses: bool,
    ) -> Vec<MaterialResult<'_>> {
        let q_lower = query.trim().to_lowercase();
        let target_courses: Vec<&Course> = if let Some(cf) = course_filter {
            self.get_course(cf).into_iter().collect()
        } else {
            self.courses(show_all_courses)
        };

        let mut results = Vec::new();
        for course in target_courses {
            for mat in &course.materials {
                let name_match = mat.name.to_lowercase().contains(&q_lower);
                let file_match = mat.filename.to_lowercase().contains(&q_lower);
                let sec_match = mat.sections.iter().any(|s| s.to_lowercase().contains(&q_lower));

                if name_match || file_match || sec_match {
                    results.push(MaterialResult {
                        material: mat,
                        course,
                    });
                }
            }
        }

        results
    }

    pub fn recent_materials(&self, days: u64, show_all_courses: bool) -> Vec<MaterialResult<'_>> {
        let target_courses = self.courses(show_all_courses);
        let mut results = Vec::new();

        for course in target_courses {
            for mat in &course.materials {
                if mat.is_new(days) {
                    results.push(MaterialResult {
                        material: mat,
                        course,
                    });
                }
            }
        }

        results.sort_by(|a, b| {
            let ts_a = a.material.timemodified.max(a.material.timecreated);
            let ts_b = b.material.timemodified.max(b.material.timecreated);
            ts_b.cmp(&ts_a)
        });

        results
    }

    pub fn get_ppt_meta(&self, fileurl: &str) -> Option<&PptMeta> {
        let mut hasher = Md5::new();
        hasher.update(fileurl.as_bytes());
        let hash = hasher.finalize().iter().map(|b| format!("{:02x}", b)).collect::<String>();
        self.ppt_meta.get(&hash)
    }

    pub fn download_dir(&self) -> PathBuf {
        if let Some(ref d) = self.config.download_dir {
            PathBuf::from(d)
        } else {
            dirs::download_dir()
                .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join("Downloads"))
                .join("gulms")
        }
    }
}
