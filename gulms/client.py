"""
GULMS Client - Main API client for Galgotias University LMS.
Supports incremental delta sync (fetching only modified courses), caching, and new material detection.
"""

import os
import sys
import json
import time
import getpass
from pathlib import Path
from typing import List, Optional, Dict, Any, Tuple
import requests

from .models import Course, Material
from .categorizer import clean_text

CONFIG_DIR = Path.home() / ".config" / "gulms"
CACHE_DIR = Path.home() / ".cache" / "gulms"

CONFIG_DIR.mkdir(parents=True, exist_ok=True)
CACHE_DIR.mkdir(parents=True, exist_ok=True)

CONFIG_FILE = CONFIG_DIR / "config.json"
CACHE_FILE = CACHE_DIR / "courses.json"
STATE_FILE = CACHE_DIR / "sync_state.json"


class GULMSClient:
    """
    High-level Python client for GULMS.
    Handles Moodle Mobile REST API authentication, course selection, and incremental caching.
    """
    def __init__(self, config_path: Path = CONFIG_FILE, cache_path: Path = CACHE_FILE):
        self.config_path = Path(config_path)
        self.cache_path = Path(cache_path)
        self.state_file = STATE_FILE
        self.config = self._load_config()
        self.base_url = self.config.get("base_url", "https://gulms.galgotiasuniversity.org").rstrip("/")
        self.token = self.config.get("token")
        self.user_id = self.config.get("user_id")
        self.selected_course_ids: List[int] = self.config.get("selected_course_ids", [])
        self.download_dir = Path(self.config.get("download_dir", str(Path.home() / "Downloads" / "gulms")))

        self.session = requests.Session()
        self.session.headers.update({
            "User-Agent": "Mozilla/5.0 (MoodleMobile; Android)",
            "Accept": "application/json"
        })

        self._all_courses_cache: Optional[List[Course]] = None

    def _load_config(self) -> dict:
        if self.config_path.exists():
            try:
                with open(self.config_path, "r", encoding="utf-8") as f:
                    return json.load(f)
            except (json.JSONDecodeError, OSError) as e:
                print(f"[WARN] Failed to load config from {self.config_path}: {e}", file=sys.stderr)
        return {}

    def _save_config(self):
        self.config["selected_course_ids"] = self.selected_course_ids
        self.config["download_dir"] = str(self.download_dir)
        try:
            self.config_path.parent.mkdir(parents=True, exist_ok=True)
            with open(self.config_path, "w", encoding="utf-8") as f:
                json.dump(self.config, f, indent=2)
        except OSError as e:
            print(f"[WARN] Failed to save config to {self.config_path}: {e}", file=sys.stderr)

    def is_setup_completed(self) -> bool:
        return bool(self.config.get("setup_completed", False))

    def mark_setup_completed(self):
        self.config["setup_completed"] = True
        self._save_config()

    def login(self, username: str = None, password: str = None) -> str:
        """Authenticate with Moodle Mobile token service."""
        username = username or self.config.get("username")
        if not username:
            username = input("Enter GULMS Student ID / Username: ").strip()
        if not password:
            password = getpass.getpass("Enter GULMS Password: ")

        token_url = f"{self.base_url}/login/token.php"
        data = {
            "username": username,
            "password": password,
            "service": "moodle_mobile_app"
        }
        res = self.session.post(token_url, data=data)
        if res.status_code != 200:
            raise RuntimeError(f"HTTP {res.status_code} during login: {res.text}")

        resp_json = res.json()
        if "token" in resp_json:
            self.token = resp_json["token"]
            self.config["token"] = self.token
            self.config["username"] = username

            site_info = self.call("core_webservice_get_site_info")
            self.user_id = site_info.get("userid")
            self.config["user_id"] = self.user_id
            self.config["fullname"] = clean_text(site_info.get("fullname"))
            self._save_config()
            return self.token
        else:
            err = resp_json.get("error", "Unknown login error")
            raise RuntimeError(f"Authentication failed: {err}")

    def call(self, wsfunction: str, **params) -> dict:
        """Call Moodle REST API function."""
        if not self.token:
            self.login()

        url = f"{self.base_url}/webservice/rest/server.php"
        data = {
            "wstoken": self.token,
            "wsfunction": wsfunction,
            "moodlewsrestformat": "json",
            **params
        }

        res = self.session.post(url, data=data)
        if res.status_code != 200:
            raise RuntimeError(f"API Error ({res.status_code}): {res.text}")

        json_data = res.json()
        if isinstance(json_data, dict) and json_data.get("exception"):
            if json_data.get("errorcode") in ("invalidtoken", "accessexception"):
                self.login()
                data["wstoken"] = self.token
                res = self.session.post(url, data=data)
                json_data = res.json()
            else:
                raise RuntimeError(f"Moodle API Exception: {json_data.get('message', json_data)}")

        return json_data

    def get_site_info(self) -> dict:
        return self.call("core_webservice_get_site_info")

    @property
    def all_courses(self) -> List[Course]:
        """All enrolled courses from the portal (cached)."""
        if self._all_courses_cache is not None:
            return self._all_courses_cache

        index = self._load_or_fetch_index()
        self._all_courses_cache = [Course(c_data, client=self) for c_data in index.values()]
        return self._all_courses_cache

    @property
    def courses(self) -> List[Course]:
        """Active/selected courses (or all courses if none specifically selected)."""
        if self.selected_course_ids:
            return [c for c in self.all_courses if c.id in self.selected_course_ids]
        return self.all_courses

    def select_courses(self, course_ids: List[int]):
        """Save a list of course IDs to be the persistent active set."""
        self.selected_course_ids = [int(cid) for cid in course_ids]
        self._save_config()

    def clear_selected_courses(self):
        """Reset course selection to include all available courses."""
        self.selected_course_ids = []
        self._save_config()

    def _load_or_fetch_index(self, refresh: bool = False) -> dict:
        if not refresh and self.cache_path.exists():
            try:
                with open(self.cache_path, "r", encoding="utf-8") as f:
                    return json.load(f)
            except (json.JSONDecodeError, OSError) as e:
                print(f"[WARN] Failed to load cache from {self.cache_path}: {e}", file=sys.stderr)

        return self.sync().get("index", {})

    def sync(self, force: bool = False, verbose: bool = True) -> Dict[str, Any]:
        """
        Incremental Delta Sync:
        Checks Moodle course timestamps in ~0.5s and only fetches contents
        for courses that were newly added or modified by faculty.
        """
        t0 = time.time()
        if not self.user_id:
            info = self.get_site_info()
            self.user_id = info.get("userid")
            self.config["user_id"] = self.user_id
            self._save_config()

        # 1. Fetch lightweight course list from Moodle (1 single fast request)
        raw_courses = self.call("core_enrol_get_users_courses", userid=self.user_id)

        # 2. Load existing cache
        cache = {}
        if self.cache_path.exists():
            try:
                with open(self.cache_path, "r", encoding="utf-8") as f:
                    cache = json.load(f)
            except (json.JSONDecodeError, OSError) as e:
                if verbose:
                    print(f"  ⚠ Failed to read cache from {self.cache_path}: {e}")

        modified_courses = []
        newly_added_files = []

        # 3. Check timestamps against cache
        for rc in raw_courses:
            cid = str(rc["id"])
            cname = clean_text(rc.get("fullname", "Unknown"))
            cshort = clean_text(rc.get("shortname", ""))
            remote_mod = rc.get("timemodified", 0)
            cached_c = cache.get(cid)

            needs_fetch = force or (cached_c is None) or (remote_mod > cached_c.get("timemodified", 0))

            if needs_fetch:
                # Existing files before update
                existing_file_keys = set()
                if cached_c:
                    for s in cached_c.get("sections", []):
                        for m in s.get("modules", []):
                            for fl in m.get("contents", []):
                                existing_file_keys.add((fl.get("filename"), fl.get("filesize", 0)))

                if verbose:
                    action = "Fetching initial" if not cached_c else "Updating modified"
                    print(f"  • {action} course: \033[1m{cshort or cname}\033[0m...")

                fetch_success = True
                try:
                    contents = self.call("core_course_get_contents", courseid=int(cid))
                except Exception as e:
                    fetch_success = False
                    if verbose:
                        print(f"  ⚠ Failed to fetch contents for {cshort or cname}: {e}")
                    contents = cached_c.get("sections", []) if cached_c else []

                if fetch_success:
                    # Detect newly added files
                    for s in contents:
                        for m in s.get("modules", []):
                            for fl in m.get("contents", []):
                                key = (fl.get("filename"), fl.get("filesize", 0))
                                if existing_file_keys and key not in existing_file_keys:
                                    newly_added_files.append({
                                        "course": cshort or cname,
                                        "section": s.get("name"),
                                        "filename": fl.get("filename"),
                                        "filesize": fl.get("filesize", 0)
                                    })

                    cache[cid] = {
                        "id": int(cid),
                        "fullname": cname,
                        "shortname": cshort,
                        "timemodified": remote_mod,
                        "sections": contents
                    }
                    modified_courses.append(cshort or cname)
            else:
                # Keep cached entry, but ensure metadata is fresh
                cached_c["fullname"] = cname
                cached_c["shortname"] = cshort
                cache[cid] = cached_c

        # 4. Save cache & sync state
        with open(self.cache_path, "w", encoding="utf-8") as f:
            json.dump(cache, f, indent=2)

        sync_state = {
            "last_sync": int(time.time()),
            "last_sync_human": time.strftime("%Y-%m-%d %H:%M:%S"),
            "modified_courses": modified_courses,
            "new_files_count": len(newly_added_files)
        }
        with open(self.state_file, "w", encoding="utf-8") as f:
            json.dump(sync_state, f, indent=2)

        self._all_courses_cache = None
        elapsed = time.time() - t0

        return {
            "index": cache,
            "checked_count": len(raw_courses),
            "modified_courses": modified_courses,
            "new_files": newly_added_files,
            "elapsed_seconds": elapsed
        }

    def get_course(self, target: str, search_all: bool = True) -> Optional[Course]:
        """Find a course by 1-based index (e.g. '1'), Moodle ID, Acronym (e.g. 'COA', 'DBMS'), or name."""
        candidate_pool = self.all_courses if search_all else self.courses
        target_str = str(target).strip()

        if target_str.isdigit():
            val = int(target_str)
            if 1 <= val <= len(candidate_pool):
                return candidate_pool[val - 1]
            for c in candidate_pool:
                if c.id == val:
                    return c

        target_lower = target_str.lower()
        target_upper = target_str.upper()

        for c in candidate_pool:
            if c.acronym and c.acronym.upper() == target_upper:
                return c

        matches = [c for c in candidate_pool if target_lower in c.fullname.lower() or target_lower in c.shortname.lower()]
        if matches:
            return matches[0]

        return None

    def search(self, query: str, course: Optional[str] = None) -> List[Material]:
        """Search materials across active courses or within a specific course."""
        q = query.lower()
        target_courses = [self.get_course(course)] if course else self.courses
        target_courses = [c for c in target_courses if c]

        results = []
        seen = set()

        for c in target_courses:
            for mat in c.materials:
                if q in mat.filename.lower() or q in mat.name.lower():
                    key = (c.id, mat.filename, mat.filesize)
                    if key not in seen:
                        seen.add(key)
                        results.append(mat)

        return results
