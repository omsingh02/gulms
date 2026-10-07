use chrono::{DateTime, Local};
use md5::{Digest, Md5};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

use crate::categorizer::{
    categorize_file, clean_display_name, clean_text, format_size, get_acronym, Category,
};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default = "default_base_url")]
    pub base_url: String,
    pub username: Option<String>,
    pub token: Option<String>,
    pub user_id: Option<u64>,
    pub fullname: Option<String>,
    pub download_dir: Option<String>,
    #[serde(default)]
    pub selected_course_ids: Vec<u64>,
    pub setup_completed: Option<bool>,
}

fn default_base_url() -> String {
    "https://gulms.galgotiasuniversity.org".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawContent {
    pub filename: Option<String>,
    pub filesize: Option<u64>,
    pub fileurl: Option<String>,
    pub timecreated: Option<u64>,
    pub timemodified: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawModule {
    pub id: Option<u64>,
    pub name: Option<String>,
    pub modname: Option<String>,
    #[serde(default)]
    pub contents: Vec<RawContent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawSection {
    pub id: Option<u64>,
    pub name: Option<String>,
    #[serde(default)]
    pub modules: Vec<RawModule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawCourse {
    pub id: u64,
    pub fullname: Option<String>,
    pub shortname: Option<String>,
    pub timemodified: Option<u64>,
    #[serde(default)]
    pub sections: Vec<RawSection>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PptMeta {
    pub lecture_num: Option<u32>,
    pub title: Option<String>,
    pub canonical_filename: Option<String>,
    #[serde(default)]
    pub topics: Vec<String>,
    pub slide_count: Option<usize>,
    pub content_hash: Option<String>,
    pub raw_filename: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncState {
    pub last_sync: Option<u64>,
    pub last_sync_human: Option<String>,
    #[serde(default)]
    pub modified_courses: Vec<String>,
    pub new_files_count: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct Material {
    pub name: String,
    pub filename: String,
    pub filesize: u64,
    pub fileurl: Option<String>,
    pub category: Category,
    pub module_type: String,
    pub sections: Vec<String>,
    pub timecreated: u64,
    pub timemodified: u64,
    pub course_id: u64,
    pub course_name: String,
    pub course_acronym: String,
}

impl Material {
    pub fn size_human(&self) -> String {
        format_size(self.filesize)
    }

    pub fn is_ppt(&self) -> bool {
        let lower = self.filename.to_lowercase();
        lower.ends_with(".pptx")
            || lower.ends_with(".ppt")
            || lower.ends_with(".pps")
            || lower.ends_with(".ppsx")
    }

    pub fn ppt_cache_key(&self) -> Option<String> {
        self.fileurl.as_ref().map(|url| {
            let mut hasher = Md5::new();
            hasher.update(url.as_bytes());
            let result = hasher.finalize();
            result.iter().map(|b| format!("{:02x}", b)).collect::<String>()
        })
    }

    pub fn date_modified_human(&self) -> String {
        let ts = if self.timemodified > 0 {
            self.timemodified
        } else {
            self.timecreated
        };
        if ts == 0 {
            return "Unknown".to_string();
        }
        if let Some(dt) = DateTime::from_timestamp(ts as i64, 0) {
            let local: DateTime<Local> = DateTime::from(dt);
            local.format("%Y-%m-%d %H:%M").to_string()
        } else {
            "Unknown".to_string()
        }
    }

    pub fn is_new(&self, days: u64) -> bool {
        let now = chrono::Utc::now().timestamp() as u64;
        let cutoff = now.saturating_sub(days * 86400);
        let ts = self.timemodified.max(self.timecreated);
        ts > cutoff
    }
}

#[derive(Debug, Clone)]
pub struct Course {
    pub id: u64,
    pub fullname: String,
    pub clean_name: String,
    pub shortname: String,
    pub acronym: String,
    pub timemodified: u64,
    pub materials: Vec<Material>,
}

#[derive(Debug, Clone)]
pub struct CanonicalSlide<'a> {
    pub lecture_num: u32,
    pub title: String,
    pub slide_count: usize,
    pub topics: Vec<String>,
    pub canonical_name: String,
    pub material: &'a Material,
}

impl Course {
    pub fn from_raw(raw: RawCourse) -> Self {
        let fullname = clean_text(&raw.fullname.unwrap_or_else(|| "Unknown Course".to_string()));
        let clean_name = clean_display_name(&fullname);
        let shortname = clean_text(&raw.shortname.unwrap_or_default());
        let acronym = get_acronym(&fullname);
        let timemodified = raw.timemodified.unwrap_or(0);

        let mut seen_materials: HashMap<(String, u64), Material> = HashMap::new();

        for s_data in raw.sections {
            let sec_name = clean_text(&s_data.name.unwrap_or_else(|| "General".to_string()));

            for mod_data in s_data.modules {
                let mod_name = clean_text(&mod_data.name.unwrap_or_default());
                let mod_type = mod_data.modname.unwrap_or_default();

                if mod_data.contents.is_empty() {
                    continue;
                }

                for f in mod_data.contents {
                    let raw_fname = f.filename.unwrap_or_else(|| mod_name.clone());
                    let fname = clean_text(&raw_fname);
                    let fsize = f.filesize.unwrap_or(0);
                    let t_mod = f.timemodified.unwrap_or(0);
                    let t_cre = f.timecreated.unwrap_or(0);
                    let key = (fname.clone(), fsize);

                    if let Some(existing) = seen_materials.get_mut(&key) {
                        if !existing.sections.contains(&sec_name) {
                            existing.sections.push(sec_name.clone());
                        }
                    } else {
                        let cat = categorize_file(&fname, fsize, &mod_type);
                        let mat = Material {
                            name: if mod_name.is_empty() { fname.clone() } else { mod_name.clone() },
                            filename: fname,
                            filesize: fsize,
                            fileurl: f.fileurl,
                            category: cat,
                            module_type: mod_type.clone(),
                            sections: vec![sec_name.clone()],
                            timecreated: t_cre,
                            timemodified: t_mod,
                            course_id: raw.id,
                            course_name: fullname.clone(),
                            course_acronym: acronym.clone(),
                        };
                        seen_materials.insert(key, mat);
                    }
                }
            }
        }

        let materials = seen_materials.into_values().collect();

        Self {
            id: raw.id,
            fullname,
            clean_name,
            shortname,
            acronym,
            timemodified,
            materials,
        }
    }

    pub fn total_size(&self) -> u64 {
        self.materials.iter().map(|m| m.filesize).sum()
    }

    pub fn total_size_human(&self) -> String {
        format_size(self.total_size())
    }

    pub fn by_category(&self) -> BTreeMap<Category, Vec<&Material>> {
        let mut grouped: BTreeMap<Category, Vec<&Material>> = BTreeMap::new();
        for m in &self.materials {
            grouped.entry(m.category).or_default().push(m);
        }
        grouped
    }

    pub fn canonical_slides<'a>(
        &'a self,
        ppt_meta: &'a HashMap<String, PptMeta>,
    ) -> (Vec<CanonicalSlide<'a>>, Vec<&'a Material>) {
        let ppt_materials: Vec<&Material> = self.materials.iter().filter(|m| m.is_ppt()).collect();
        let mut by_lec: BTreeMap<u32, Vec<(&'a Material, &'a PptMeta)>> = BTreeMap::new();
        let mut unnumbered: Vec<&'a Material> = Vec::new();

        for m in ppt_materials {
            if let Some(key) = m.ppt_cache_key() {
                if let Some(meta) = ppt_meta.get(&key) {
                    if let Some(num) = meta.lecture_num {
                        by_lec.entry(num).or_default().push((m, meta));
                        continue;
                    }
                }
            }
            unnumbered.push(m);
        }

        let mut canonical = Vec::new();
        for (num, list) in by_lec {
            // Pick best slide by slide_count, then topics count
            let (best_m, best_meta) = list
                .into_iter()
                .max_by_key(|(_, meta)| {
                    (
                        meta.slide_count.unwrap_or(0),
                        meta.topics.len(),
                    )
                })
                .unwrap();

            canonical.push(CanonicalSlide {
                lecture_num: num,
                title: best_meta.title.clone().unwrap_or_else(|| best_m.name.clone()),
                slide_count: best_meta.slide_count.unwrap_or(0),
                topics: best_meta.topics.clone(),
                canonical_name: best_meta
                    .canonical_filename
                    .clone()
                    .unwrap_or_else(|| best_m.filename.clone()),
                material: best_m,
            });
        }

        (canonical, unnumbered)
    }
}
