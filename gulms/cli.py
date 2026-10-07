"""
GULMS CLI - Clean command-line interface for Galgotias University LMS.
"""

import os
import sys
import argparse
import getpass
import re
from datetime import datetime
from pathlib import Path
from typing import List, Dict, Optional, Tuple

from .client import GULMSClient, CONFIG_FILE
from .models import Course, Material
from .ppt_analyzer import PPTInfo
from .downloader import format_size, sanitize_filename, download_file
from .categorizer import clean_text

# Terminal colors (auto-disabled if output is piped)
if sys.stdout.isatty():
    RESET = "\033[0m"
    BOLD = "\033[1m"
    DIM = "\033[2m"
    CYAN = "\033[36m"
    GREEN = "\033[32m"
    YELLOW = "\033[33m"
    RED = "\033[31m"
else:
    RESET = BOLD = DIM = CYAN = GREEN = YELLOW = RED = ""


def clean_display_name(raw_name: str) -> str:
    """Strip redundant course code from name e.g. 'Data Base management System(E2UC302B)' -> 'Data Base management System'."""
    cleaned = re.sub(r'\s*\([A-Z0-9_-]+\)\s*$', '', raw_name).strip()
    return cleaned or raw_name


def parse_selection_input(raw: str, max_val: int) -> List[int]:
    """Parse comma/space/range inputs like '1, 2, 4-6'."""
    tokens = [t.strip() for t in raw.replace(",", " ").split() if t.strip()]
    selected = set()
    for token in tokens:
        if "-" in token:
            parts = token.split("-")
            if len(parts) == 2 and parts[0].isdigit() and parts[1].isdigit():
                start, end = int(parts[0]), int(parts[1])
                for i in range(min(start, end), max(start, end) + 1):
                    if 1 <= i <= max_val:
                        selected.add(i)
        elif token.isdigit():
            val = int(token)
            if 1 <= val <= max_val:
                selected.add(val)
    return sorted(list(selected))


def guided_setup(client: GULMSClient):
    """Clean interactive setup."""
    print(f"\n{BOLD}GULMS Setup{RESET}\n")

    # 1. Auth
    if not client.token:
        print("[1/3] Credentials")
        username = input("Username / Admission No: ").strip()
        password = getpass.getpass("Password: ")
        try:
            client.login(username, password)
            print(f"Authenticated as {client.get_site_info().get('fullname')}\n")
        except Exception as e:
            print(f"{RED}Login failed:{RESET} {e}")
            sys.exit(1)
    else:
        info = client.get_site_info()
        print(f"[1/3] Authenticated as {info.get('fullname')} ({info.get('username')})\n")

    # 2. Select Courses
    all_courses = client.all_courses
    print(f"[2/3] Active Courses ({len(all_courses)} available):\n")
    for i, c in enumerate(all_courses, start=1):
        acronym = f"[{c.acronym}]" if c.acronym else "      "
        cname = clean_display_name(c.name)
        print(f"  {i:>2}  {acronym:<7} {cname}")

    while True:
        choice = input("\nSelect active courses (e.g. 1, 2, 4-6 | 'all'): ").strip().lower()
        if choice in ("all", "*"):
            client.clear_selected_courses()
            break
        indices = parse_selection_input(choice, len(all_courses))
        if indices:
            selected_cids = [all_courses[i - 1].id for i in indices]
            client.select_courses(selected_cids)
            print(f"Saved {len(selected_cids)} courses.")
            break
        print("Invalid selection.")

    # 3. Download directory
    default_dir = Path.home() / "Downloads" / "gulms"
    print(f"\n[3/3] Download Directory")
    dir_input = input(f"Destination [{default_dir}]: ").strip()
    target_dir = Path(os.path.expanduser(dir_input)) if dir_input else default_dir
    target_dir.mkdir(parents=True, exist_ok=True)
    client.download_dir = target_dir
    client.config["download_dir"] = str(target_dir)

    client.mark_setup_completed()
    print(f"\n{GREEN}Setup complete.{RESET}\n")


def cmd_whoami(client: GULMSClient, args):
    info = client.get_site_info()
    print(f"User:     {info.get('fullname')} ({info.get('userid')})")
    print(f"Username: {info.get('username')}")
    print(f"Site:     {info.get('sitename')}")


def cmd_select(client: GULMSClient, args):
    all_courses = client.all_courses
    current_selected = set(client.selected_course_ids)

    print(f"\nCourses ({len(all_courses)}):")
    for i, c in enumerate(all_courses, start=1):
        mark = "[✓]" if c.id in current_selected else "[ ]"
        acronym = f"[{c.acronym}]" if c.acronym else "      "
        cname = clean_display_name(c.name)
        print(f"  {i:>2}  {mark}  {acronym:<7} {cname}")

    print()
    choice = input("Select active courses (e.g. 1, 2, 4-6 | 'all' | 'clear'): ").strip().lower()
    if not choice:
        return

    if choice in ("all", "*"):
        client.clear_selected_courses()
        print("Selected all courses.\n")
        return

    if choice in ("clear", "reset", "none"):
        client.clear_selected_courses()
        print("Cleared selection.\n")
        return

    indices = parse_selection_input(choice, len(all_courses))
    if not indices:
        print("No valid courses selected.")
        return

    selected_cids = [all_courses[i - 1].id for i in indices]
    client.select_courses(selected_cids)
    print(f"Saved {len(selected_cids)} active courses.\n")


def cmd_courses(client: GULMSClient, args):
    if args.refresh:
        client.sync(verbose=False)

    show_all = getattr(args, "all", False)
    courses = client.all_courses if show_all else client.courses
    header = f"All Courses ({len(courses)}):" if show_all else f"Active Courses ({len(courses)}):"

    print(f"\n{header}")
    for i, c in enumerate(courses, start=1):
        acronym = f"[{c.acronym}]" if c.acronym else "      "
        code = f"({c.shortname})" if c.shortname else ""
        cname = clean_display_name(c.name)
        print(f"  {i:>2}  {acronym:<7} {cname:<52} {DIM}{code}{RESET}")
    print()


def cmd_view(client: GULMSClient, args):
    if args.refresh:
        client.sync(verbose=False)
    course = client.get_course(args.course)
    if not course:
        print(f"Course '{args.course}' not found.")
        sys.exit(1)

    cname = clean_display_name(course.name)
    print(f"\n{BOLD}{course.acronym or course.name}{RESET} — {cname} ({course.total_size_human})\n")

    if args.by_section:
        for sec in course.sections:
            if sec.is_empty:
                continue
            print(f"{BOLD}{sec.name}{RESET} ({len(sec.materials)} files)")
            for m in sec.materials:
                print(f"  {m.filename} {DIM}({m.size_human}){RESET}")
            print()
        return

    cats = course.categories
    for cat_name, items in cats.items():
        if not items:
            continue
        cat_bytes = sum(x.filesize for x in items)

        # Consolidate PPT slides in view mode
        if "Slides" in cat_name:
            by_lec = {}
            unnum = []
            for m in items:
                inf = m.get_ppt_info()
                if inf and inf.lecture_num is not None:
                    by_lec.setdefault(inf.lecture_num, []).append((m, inf))
                else:
                    unnum.append(m)

            canonical_items = []
            for n in sorted(by_lec.keys()):
                best_m, _ = max(by_lec[n], key=lambda x: (x[1].slide_count, len(x[1].topics)))
                canonical_items.append(best_m)
            canonical_items.extend(unnum)

            print(f"{BOLD}{cat_name}{RESET} ({len(canonical_items)} lectures, {format_size(cat_bytes)})")
            for m in canonical_items:
                print(f"  {m.canonical_name} {DIM}({m.size_human}){RESET}")
            print()
        else:
            show_all_files = getattr(args, "all", False)
            display_items = items if (show_all_files or len(items) <= 15) else items[:12]
            print(f"{BOLD}{cat_name}{RESET} ({len(items)} files, {format_size(cat_bytes)})")
            for m in display_items:
                print(f"  {m.filename} {DIM}({m.size_human}){RESET}")
            if len(items) > len(display_items):
                print(f"  {DIM}... and {len(items) - len(display_items)} more files (pass -a to show all){RESET}")
            print()


def cmd_slides(client: GULMSClient, args):
    if args.refresh:
        client.sync(verbose=False)
    course = client.get_course(args.course)
    if not course:
        print(f"Course '{args.course}' not found.")
        sys.exit(1)

    ppt_slides = [m for m in course.materials if m.is_ppt]
    if not ppt_slides:
        print(f"No slides found for {course.name}.")
        return

    slides_with_info: List[Tuple[Material, Optional[PPTInfo]]] = []
    by_lec: Dict[int, List[Tuple[Material, PPTInfo]]] = {}
    unnumbered: List[Tuple[Material, PPTInfo]] = []

    for mat in ppt_slides:
        info = mat.get_ppt_info()
        slides_with_info.append((mat, info))
        if info:
            if info.lecture_num is not None:
                by_lec.setdefault(info.lecture_num, []).append((mat, info))
            else:
                unnumbered.append((mat, info))

    show_all = getattr(args, "all", False)
    show_outline = getattr(args, "outline", False)

    if show_all:
        print(f"\n{clean_display_name(course.name)} — All Slides ({len(ppt_slides)} files)\n")
        print(f"  {'LEC':<8} {'FILE':<52} {'SLIDES'}")
        print(f"  {'─'*8} {'─'*52} {'─'*6}")
        for mat, info in slides_with_info:
            lec_str = f"Lec-{info.lecture_num:02d}" if (info and info.lecture_num is not None) else "Lec-??"
            fname = (info.canonical_filename if info else mat.filename)[:50]
            scount = str(info.slide_count) if info else "?"
            print(f"  {CYAN}{lec_str:<8}{RESET} {fname:<52} {scount}")
        print()
        return

    valid_nums = sorted(by_lec.keys())
    cname = clean_display_name(course.name)
    print(f"\n{BOLD}{course.acronym or course.name}{RESET} — {cname} ({len(valid_nums)} lectures)\n")

    for num in valid_nums:
        versions = by_lec[num]
        best_mat, best_info = max(versions, key=lambda x: (x[1].slide_count, len(x[1].topics)))

        lec_label = f"Lec-{num:02d}"
        title = best_info.title
        if len(title) > 54:
            title = title[:52] + ".."

        print(f"  {CYAN}{lec_label}{RESET}  {title:<54}  {best_info.slide_count:>2} slides")
        if show_outline and best_info.topics:
            topics_preview = " • ".join(best_info.topics[:4])
            print(f"          {DIM}Outline: {topics_preview}{RESET}")

    if unnumbered:
        print(f"\n  {DIM}Other:{RESET}")
        for mat, info in unnumbered:
            title = info.title[:50]
            print(f"    {title:<56}  {info.slide_count:>2} slides")
    print()




def cmd_notes(client: GULMSClient, args):
    course = client.get_course(args.course)
    if not course:
        print(f"Course '{args.course}' not found.")
        sys.exit(1)

    ppt_slides = [m for m in course.materials if m.is_ppt]
    if not ppt_slides:
        print(f"No lecture slides found for {course.name}.")
        return

    # Map by lecture number
    by_lec = {}
    unnumbered = []
    for mat in ppt_slides:
        info = mat.get_ppt_info()
        if info:
            if info.lecture_num is not None:
                by_lec.setdefault(info.lecture_num, []).append((mat, info))
            else:
                unnumbered.append((mat, info))

    targets = []
    if args.lecture is not None:
        if args.lecture not in by_lec:
            print(f"Lecture {args.lecture} not found in {course.acronym or course.name}.")
            print(f"Available lectures: {sorted(by_lec.keys())}")
            return
        best_mat, best_info = max(by_lec[args.lecture], key=lambda x: (x[1].slide_count, len(x[1].topics)))
        targets.append((best_mat, best_info))
    else:
        for num in sorted(by_lec.keys()):
            best_mat, best_info = max(by_lec[num], key=lambda x: (x[1].slide_count, len(x[1].topics)))
            targets.append((best_mat, best_info))

    out_dir = Path(args.dest)
    cname = clean_display_name(course.name)
    print(f"\n{BOLD}Exporting study notes for {course.acronym or course.name}{RESET} ({len(targets)} lectures)...")

    for mat, info in targets:
        lec_label = f"Lec-{info.lecture_num:02d}" if info.lecture_num else "Lec-??"
        print(f"  {CYAN}{lec_label:<8}{RESET} {info.title[:45]:<45} ... ", end="", flush=True)
        try:
            as_pdf = getattr(args, "pdf", True)
            open_v = getattr(args, "open", False)
            out_file = mat.export_notes(dest_dir=out_dir, as_pdf=as_pdf, open_viewer=open_v)
            if out_file and out_file.exists():
                print(f"{GREEN}✓ Saved to {out_file.name}{RESET}")
            else:
                print(f"{YELLOW}Failed{RESET}")
        except Exception as e:
            print(f"{RED}Error: {e}{RESET}")

    print(f"\nNotes saved to: {BOLD}{out_dir.resolve()}{RESET}\n")


def cmd_open(client: GULMSClient, args):
    """Opens a course lecture note PDF directly in Zathura."""
    course = client.get_course(args.course)
    if not course:
        print(f"Course '{args.course}' not found.")
        sys.exit(1)

    ppt_slides = [m for m in course.materials if m.is_ppt]
    if not ppt_slides:
        print(f"No lecture slides found for {course.name}.")
        return

    # Map by lecture number
    by_lec = {}
    for mat in ppt_slides:
        info = mat.get_ppt_info()
        if info and info.lecture_num is not None:
            by_lec.setdefault(info.lecture_num, []).append((mat, info))

    if not by_lec:
        print(f"No numbered lectures found for {course.acronym or course.name}.")
        return

    lec_num = args.lecture
    if lec_num is None:
        avail = sorted(by_lec.keys())
        print(f"\n{BOLD}Lectures in {course.acronym or course.name}:{RESET}")
        for num in avail:
            best_mat, best_info = max(by_lec[num], key=lambda x: (x[1].slide_count, len(x[1].topics)))
            print(f"  {CYAN}Lec-{num:02d}{RESET}  {best_info.title[:50]}")
        try:
            choice = input(f"\nSelect lecture number to open [{avail[0]}]: ").strip()
            lec_num = int(choice) if choice else avail[0]
        except (ValueError, EOFError, KeyboardInterrupt):
            print()
            return

    if lec_num not in by_lec:
        print(f"Lecture {lec_num} not found in {course.acronym or course.name}.")
        print(f"Available lectures: {sorted(by_lec.keys())}")
        return

    best_mat, best_info = max(by_lec[lec_num], key=lambda x: (x[1].slide_count, len(x[1].topics)))
    out_dir = Path(args.dest)
    course_dir = out_dir / sanitize_filename(course.shortname or course.name)
    canonical_stem = Path(best_mat.canonical_name).stem
    expected_pdf = course_dir / f"{canonical_stem}.pdf"

    if expected_pdf.exists():
        print(f"Opening {BOLD}{expected_pdf.name}{RESET} in Zathura...")
        from .pdf_renderer import open_in_zathura
        open_in_zathura(expected_pdf)
    else:
        print(f"Generating vector PDF for Lec-{lec_num:02d} ({best_info.title[:45]})...")
        pdf_file = best_mat.export_notes(dest_dir=out_dir, as_pdf=True, open_viewer=True)
        if pdf_file and pdf_file.exists():
            print(f"{GREEN}✓ Opened in Zathura ({pdf_file.name}){RESET}")
        else:
            print(f"{RED}Failed to generate PDF.{RESET}")


def cmd_sync(client: GULMSClient, args):
    res = client.sync(force=args.force, verbose=False)
    elapsed = res["elapsed_seconds"]
    mod_courses = res["modified_courses"]
    new_files = res["new_files"]

    if not mod_courses and not new_files:
        print(f"Up to date (checked {res['checked_count']} courses in {elapsed:.1f}s)")
    else:
        print(f"Synced in {elapsed:.1f}s:")
        if mod_courses:
            for cname in mod_courses:
                print(f"  • {cname}")
        if new_files:
            for nf in new_files:
                print(f"    + [{nf['course']}] {nf['filename']} ({format_size(nf['filesize'])})")


def cmd_new(client: GULMSClient, args):
    days = args.days or 7
    cutoff = datetime.now().timestamp() - (days * 86400)

    all_recent = []
    for c in client.courses:
        for m in c.materials:
            t = m.timemodified or m.timecreated
            if t and t > cutoff:
                all_recent.append(m)

    all_recent.sort(key=lambda x: (x.timemodified or x.timecreated), reverse=True)

    if not all_recent:
        print(f"No uploads in the last {days} days.")
        return

    print(f"\nRecent uploads (last {days} days):")
    for m in all_recent[:20]:
        dt_str = datetime.fromtimestamp(m.timemodified or m.timecreated).strftime("%b %d")
        c_code = m.course.acronym or m.course.shortname if m.course else ""
        print(f"  [{c_code}] {m.filename} {DIM}({m.size_human}, {dt_str}){RESET}")
    print()

    if args.dest:
        dest = Path(args.dest)
        print(f"Downloading {len(all_recent)} files to {dest}...")
        for m in all_recent:
            m.download(dest_dir=dest, show_progress=False)


def cmd_search(client: GULMSClient, args):
    results = client.search(args.query, course=args.course)
    if not results:
        print(f"No files matched '{args.query}'.")
        return

    print(f"\nResults for '{args.query}' ({len(results)}):")
    for i, m in enumerate(results[:25], start=1):
        c_code = m.course.acronym or m.course.shortname if m.course else ""
        print(f"  {i:>2}  [{c_code}] {m.filename} {DIM}({m.size_human}){RESET}")
    print()

    if args.download:
        dest = Path(args.dest or client.download_dir)
        print(f"Downloading {len(results)} files to {dest}...")
        for m in results:
            m.download(dest_dir=dest, show_progress=False)


def cmd_download(client: GULMSClient, args):
    dest_base = Path(args.dest or client.download_dir)
    results = client.search(args.query)

    if not results:
        print(f"No files matched '{args.query}'.")
        return

    if len(results) == 1 or args.yes:
        selected = results
    else:
        print(f"\nMatches for '{args.query}':")
        for i, m in enumerate(results[:10], start=1):
            c_name = m.course.acronym or m.course.shortname if m.course else ""
            print(f"  {i:>2}  [{c_name}] {m.filename} ({m.size_human})")

        choice = input("\nSelect file to download (or 'all'): ").strip().lower()
        if choice in ("all", "*"):
            selected = results
        else:
            try:
                idx = int(choice)
                selected = [results[idx - 1]]
            except Exception:
                return

    for m in selected:
        name = m.canonical_name if m.is_ppt else m.filename
        print(f"Downloading {name} ({m.size_human})...", end="", flush=True)
        m.download(dest_dir=dest_base, show_progress=False, use_canonical_name=True)
        print(" done")


def cmd_download_course(client: GULMSClient, args):
    if args.refresh:
        client.sync(verbose=False)
    course = client.get_course(args.course)
    if not course:
        print(f"Course '{args.course}' not found.")
        sys.exit(1)

    dest_base = Path(args.dest or client.download_dir) / (course.acronym or str(course.id))
    dest_base.mkdir(parents=True, exist_ok=True)

    # Master curriculum: deduplicate PPTs to canonical lectures
    ppt_slides = [m for m in course.materials if m.is_ppt]
    non_ppt = [m for m in course.materials if not m.is_ppt]

    by_lec = {}
    unnumbered = []
    for m in ppt_slides:
        info = m.get_ppt_info()
        if info and info.lecture_num is not None:
            by_lec.setdefault(info.lecture_num, []).append((m, info))
        else:
            unnumbered.append(m)

    canonical_slides = []
    for num, vers in by_lec.items():
        best_m, _ = max(vers, key=lambda x: (x[1].slide_count, len(x[1].topics)))
        canonical_slides.append(best_m)

    files_to_dl = canonical_slides + unnumbered + non_ppt

    if not files_to_dl:
        print("No files found.")
        return

    total_bytes = sum(m.filesize for m in files_to_dl)
    print(f"\nDownloading {course.acronym or course.name} ({len(files_to_dl)} files, {format_size(total_bytes)}) to {dest_base}")

    if not args.yes:
        act = input("Proceed? [y/N]: ").strip().lower()
        if act != 'y':
            return

    for i, m in enumerate(files_to_dl, start=1):
        target_name = m.canonical_name if m.is_ppt else m.filename
        cat_dir = dest_base / sanitize_filename(m.category)
        cat_dir.mkdir(parents=True, exist_ok=True)
        target_file = cat_dir / sanitize_filename(target_name)

        if target_file.exists() and target_file.stat().st_size == m.filesize:
            print(f"  [{i:>2}/{len(files_to_dl)}] {target_name} {DIM}(cached){RESET}")
            continue

        print(f"  [{i:>2}/{len(files_to_dl)}] {target_name} ({m.size_human})...", end="", flush=True)
        download_file(
            session=client.session,
            file_url=m.fileurl,
            token=client.token,
            dest_path=cat_dir,
            filename=target_name,
            show_progress=False
        )
        print(" done")

    print(f"\nDownloaded {len(files_to_dl)} files to {dest_base}\n")


def interactive_mode(client: GULMSClient):
    num_selected = len(client.selected_course_ids)
    courses_label = f"{num_selected} active courses" if num_selected else "all courses"

    while True:
        print(f"\nGULMS ({courses_label})\n")
        print("  1. Courses")
        print("  2. Slides")
        print("  3. Files")
        print("  4. Recent uploads")
        print("  5. Sync")
        print("  6. Download course")
        print("  7. Select active courses")
        print("  8. Whoami")
        print("  0. Exit")

        choice = input("\nChoice [0-8]: ").strip()
        if choice in ("0", "q", "exit"):
            break
        elif choice == "1":
            class DummyArgs:
                refresh = False
                all = False
            cmd_courses(client, DummyArgs())
        elif choice == "2":
            c_target = input("Course acronym or number: ").strip()
            if c_target:
                class DummyArgs:
                    course = c_target
                    refresh = False
                    all = False
                    outline = False
                cmd_slides(client, DummyArgs())
        elif choice == "3":
            c_target = input("Course acronym or number: ").strip()
            if c_target:
                class DummyArgs:
                    course = c_target
                    refresh = False
                    by_section = False
                cmd_view(client, DummyArgs())
        elif choice == "4":
            class DummyArgs:
                days = 7
                dest = None
            cmd_new(client, DummyArgs())
        elif choice == "5":
            class DummyArgs:
                force = False
            cmd_sync(client, DummyArgs())
        elif choice == "6":
            c_target = input("Course acronym or number to download: ").strip()
            if c_target:
                class DummyArgs:
                    course = c_target
                    dest = None
                    refresh = False
                    yes = False
                cmd_download_course(client, DummyArgs())
        elif choice == "7":
            class DummyArgs: pass
            cmd_select(client, DummyArgs())
            num_selected = len(client.selected_course_ids)
            courses_label = f"{num_selected} active courses" if num_selected else "all courses"
        elif choice == "8":
            class DummyArgs: pass
            cmd_whoami(client, DummyArgs())


def main():
    parser = argparse.ArgumentParser(
        prog="gulms",
        description="Galgotias University LMS CLI",
    )
    parser.add_argument("-i", "--interactive", action="store_true", help="Interactive menu")
    subparsers = parser.add_subparsers(dest="command")

    subparsers.add_parser("whoami", help="Show user profile")
    subparsers.add_parser("setup", help="Run guided setup")
    subparsers.add_parser("select", help="Choose active courses")

    p_sync = subparsers.add_parser("sync", help="Sync course cache")
    p_sync.add_argument("-f", "--force", action="store_true", help="Force full re-sync")

    p_new = subparsers.add_parser("new", help="List recent uploads")
    p_new.add_argument("-d", "--days", type=int, default=7, help="Days to look back (default: 7)")
    p_new.add_argument("--dest", help="Destination directory to download")

    p_courses = subparsers.add_parser("courses", aliases=["ls"], help="List courses")
    p_courses.add_argument("-a", "--all", action="store_true", help="List all enrolled courses")
    p_courses.add_argument("-r", "--refresh", action="store_true", help="Refresh cache")

    p_view = subparsers.add_parser("view", aliases=["cat"], help="List files in course")
    p_view.add_argument("course", help="Course acronym or number")
    p_view.add_argument("-a", "--all", action="store_true", help="Show all files without truncation")
    p_view.add_argument("-r", "--refresh", action="store_true", help="Refresh cache")
    p_view.add_argument("--by-section", action="store_true", help="Group by section")

    p_slides = subparsers.add_parser("slides", help="List lecture slides")
    p_slides.add_argument("course", help="Course acronym or number")
    p_slides.add_argument("-a", "--all", action="store_true", help="Show all raw professor uploads")
    p_slides.add_argument("-o", "--outline", action="store_true", help="Show lecture agenda/outlines")
    p_slides.add_argument("-r", "--refresh", action="store_true", help="Refresh cache")
    p_slides.add_argument("--export-md", action="store_true", help="Export lecture slides to clean Markdown notes")
    p_slides.add_argument("--dest", default="./notes", help="Destination folder for exported notes")

    p_notes = subparsers.add_parser("notes", help="Extract study notes (Markdown & PDF) from lecture slides")
    p_notes.add_argument("course", help="Course acronym or number")
    p_notes.add_argument("lecture", nargs="?", type=int, help="Optional lecture number (e.g. 1, 2, 3)")
    p_notes.add_argument("--dest", default="./notes", help="Destination folder (default: ./notes)")
    p_notes.add_argument("--pdf", action="store_true", default=True, help="Render vector PDF alongside Markdown (default: True)")
    p_notes.add_argument("--no-pdf", dest="pdf", action="store_false", help="Do not render PDF, keep Markdown only")
    p_notes.add_argument("-o", "--open", action="store_true", help="Automatically open generated PDF in Zathura")

    p_open = subparsers.add_parser("open", help="Open lecture notes PDF in Zathura")
    p_open.add_argument("course", help="Course acronym or number")
    p_open.add_argument("lecture", nargs="?", type=int, help="Optional lecture number (e.g. 1, 2, 3)")
    p_open.add_argument("--dest", default="./notes", help="Destination folder (default: ./notes)")

    p_search = subparsers.add_parser("search", help="Search materials")
    p_search.add_argument("query", help="Search keyword")
    p_search.add_argument("-c", "--course", help="Filter by course")
    p_search.add_argument("-d", "--download", action="store_true", help="Download matches")
    p_search.add_argument("--dest", help="Destination folder")

    p_dl = subparsers.add_parser("download", aliases=["get"], help="Download a file or search result")
    p_dl.add_argument("query", help="Filename or keyword")
    p_dl.add_argument("--dest", help="Destination folder")
    p_dl.add_argument("-y", "--yes", action="store_true", help="Skip confirmation")

    p_dlc = subparsers.add_parser("download-course", help="Download all files for a course")
    p_dlc.add_argument("course", help="Course acronym or number")
    p_dlc.add_argument("--dest", help="Destination folder")
    p_dlc.add_argument("-y", "--yes", action="store_true", help="Skip confirmation")

    args = parser.parse_args()
    client = GULMSClient()

    if not client.is_setup_completed() and (not args.command or args.command == "setup"):
        guided_setup(client)
        return

    if args.command == "setup":
        guided_setup(client)
        return

    if args.interactive or not args.command:
        interactive_mode(client)
        return

    cmd = args.command
    if cmd == "whoami":
        cmd_whoami(client, args)
    elif cmd == "select":
        cmd_select(client, args)
    elif cmd == "sync":
        cmd_sync(client, args)
    elif cmd == "new":
        cmd_new(client, args)
    elif cmd in ("courses", "ls"):
        cmd_courses(client, args)
    elif cmd in ("view", "cat"):
        cmd_view(client, args)
    elif cmd == "slides":
        if getattr(args, "export_md", False):
            setattr(args, "lecture", None)
            cmd_notes(client, args)
        else:
            cmd_slides(client, args)
    elif cmd == "notes":
        cmd_notes(client, args)
    elif cmd == "open":
        cmd_open(client, args)
    elif cmd == "search":
        cmd_search(client, args)
    elif cmd in ("download", "get"):
        cmd_download(client, args)
    elif cmd == "download-course":
        cmd_download_course(client, args)
    else:
        parser.print_help()


if __name__ == "__main__":
    main()
