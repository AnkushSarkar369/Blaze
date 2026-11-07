#!/usr/bin/env python3

import argparse
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


# ─────────────────────────────────────────────────────────────────────────────
# Configuration
# ─────────────────────────────────────────────────────────────────────────────

RESET = "\033[0m"
BOLD = "\033[1m"
DIM = "\033[2m"

RED = "\033[31m"
GREEN = "\033[32m"
YELLOW = "\033[33m"
CYAN = "\033[36m"
BLUE = "\033[34m"
MAGENTA = "\033[35m"


def c(text, colour):
    return f"{colour}{text}{RESET}"


def die(message):
    print(f"\n{c('ERROR', RED)} {message}\n")
    sys.exit(1)


def check_dependency(name):
    if shutil.which(name) is None:
        die(
            f"{name} was not found in PATH.\n"
            f"Install FFmpeg to use this script."
        )


# ─────────────────────────────────────────────────────────────────────────────
# Time handling
# ─────────────────────────────────────────────────────────────────────────────

def parse_time(value):
    """
    Accept:
        SS
        MM:SS
        HH:MM:SS
    """

    value = value.strip()

    try:
        parts = [int(x) for x in value.split(":")]
    except ValueError:
        raise ValueError(f"Invalid timestamp: {value}")

    if len(parts) == 1:
        return parts[0]

    if len(parts) == 2:
        minutes, seconds = parts
        if seconds >= 60:
            raise ValueError(f"Invalid timestamp: {value}")
        return minutes * 60 + seconds

    if len(parts) == 3:
        hours, minutes, seconds = parts
        if minutes >= 60 or seconds >= 60:
            raise ValueError(f"Invalid timestamp: {value}")
        return hours * 3600 + minutes * 60 + seconds

    raise ValueError(f"Invalid timestamp: {value}")


def format_time(seconds):
    seconds = max(0, int(round(seconds)))

    hours = seconds // 3600
    minutes = (seconds % 3600) // 60
    secs = seconds % 60

    if hours:
        return f"{hours:02d}:{minutes:02d}:{secs:02d}"

    return f"{minutes:02d}:{secs:02d}"


# ─────────────────────────────────────────────────────────────────────────────
# FFprobe
# ─────────────────────────────────────────────────────────────────────────────

def get_duration(video):
    result = subprocess.run(
        [
            "ffprobe",
            "-v", "error",
            "-show_entries", "format=duration",
            "-of", "default=noprint_wrappers=1:nokey=1",
            str(video),
        ],
        capture_output=True,
        text=True,
    )

    if result.returncode != 0:
        die(f"Could not read duration of:\n{video}")

    try:
        return float(result.stdout.strip())
    except ValueError:
        die(f"Could not determine duration of:\n{video}")


# ─────────────────────────────────────────────────────────────────────────────
# cuts.txt
# ─────────────────────────────────────────────────────────────────────────────

def read_cuts(path, duration):
    cuts = []

    with open(path, "r", encoding="utf-8") as f:
        for line_number, raw_line in enumerate(f, 1):

            line = raw_line.strip()

            # Blank lines and comments are allowed.
            if not line or line.startswith("#"):
                continue

            if "-" not in line:
                raise ValueError(
                    f"Line {line_number}: expected START-END\n"
                    f"Got: {line}"
                )

            start_text, end_text = line.split("-", 1)

            try:
                start = parse_time(start_text)
                end = parse_time(end_text)
            except ValueError as e:
                raise ValueError(f"Line {line_number}: {e}")

            if start < 0:
                raise ValueError(
                    f"Line {line_number}: start cannot be negative."
                )

            if end <= start:
                raise ValueError(
                    f"Line {line_number}: end must be after start."
                )

            if start >= duration:
                raise ValueError(
                    f"Line {line_number}: start {format_time(start)} "
                    f"is beyond the video duration ({format_time(duration)})."
                )

            # Clamp a final cut that extends microscopically past EOF.
            end = min(end, duration)

            cuts.append((start, end))

    cuts.sort()

    # Merge overlapping / touching cuts.
    merged = []

    for start, end in cuts:
        if not merged:
            merged.append([start, end])
            continue

        previous_start, previous_end = merged[-1]

        if start <= previous_end:
            merged[-1][1] = max(previous_end, end)
        else:
            merged.append([start, end])

    return [(start, end) for start, end in merged]


# ─────────────────────────────────────────────────────────────────────────────
# DELETE → KEEP
# ─────────────────────────────────────────────────────────────────────────────

def calculate_keep_ranges(cuts, duration):
    keep = []
    cursor = 0

    for cut_start, cut_end in cuts:

        if cut_start > cursor:
            keep.append((cursor, cut_start))

        cursor = max(cursor, cut_end)

    if cursor < duration:
        keep.append((cursor, duration))

    return keep


# ─────────────────────────────────────────────────────────────────────────────
# Human-readable summary
# ─────────────────────────────────────────────────────────────────────────────

def print_ranges(title, ranges, colour):
    print(f"\n{c(title, colour)}")

    if not ranges:
        print(f"  {DIM}(none){RESET}")
        return

    for i, (start, end) in enumerate(ranges, 1):
        duration = end - start

        print(
            f"  {c(f'{i:02d}', DIM)}  "
            f"{format_time(start)} → {format_time(end)}  "
            f"{DIM}({format_time(duration)}){RESET}"
        )


# ─────────────────────────────────────────────────────────────────────────────
# FFmpeg runner
# ─────────────────────────────────────────────────────────────────────────────

def run_ffmpeg(command, description):
    print(f"\n{c('▶', CYAN)} {description}")

    process = subprocess.run(command)

    if process.returncode != 0:
        die(
            f"FFmpeg failed while {description.lower()}.\n"
            f"Nothing was done to the original source."
        )


# ─────────────────────────────────────────────────────────────────────────────
# Main
# ─────────────────────────────────────────────────────────────────────────────

def main():

    parser = argparse.ArgumentParser(
        description=(
            "Lossless second-precision video cutter. "
            "Ranges in cuts.txt are DELETE ranges."
        )
    )

    parser.add_argument(
        "video",
        help="Source video"
    )

    parser.add_argument(
        "cuts",
        help="Text file containing DELETE ranges"
    )

    parser.add_argument(
        "-o",
        "--output",
        help="Output filename (default: SOURCE_cut.EXT)"
    )

    parser.add_argument(
        "--keep-temp",
        action="store_true",
        help="Keep temporary clips after completion"
    )

    args = parser.parse_args()

    check_dependency("ffmpeg")
    check_dependency("ffprobe")

    video = Path(args.video).expanduser().resolve()
    cuts_file = Path(args.cuts).expanduser().resolve()

    if not video.is_file():
        die(f"Source video does not exist:\n{video}")

    if not cuts_file.is_file():
        die(f"cuts.txt does not exist:\n{cuts_file}")

    # ─────────────────────────────────────────────────────────────────────────
    # Source information
    # ─────────────────────────────────────────────────────────────────────────

    print()
    print(c("=" * 64, BLUE))
    print(c(" LOSSLESS VIDEO CUTTER ", BOLD + BLUE))
    print(c("=" * 64, BLUE))

    print(f"\n{c('SOURCE', CYAN)}")
    print(f"  {video}")

    duration = get_duration(video)

    print(f"  Duration: {c(format_time(duration), BOLD)}")

    # ─────────────────────────────────────────────────────────────────────────
    # Read DELETE ranges
    # ─────────────────────────────────────────────────────────────────────────

    try:
        cuts = read_cuts(cuts_file, duration)
    except ValueError as e:
        die(str(e))

    if not cuts:
        die("cuts.txt contains no valid cut ranges.")

    print_ranges("DELETE", cuts, RED)

    # ─────────────────────────────────────────────────────────────────────────
    # Calculate KEEP ranges
    # ─────────────────────────────────────────────────────────────────────────

    keep = calculate_keep_ranges(cuts, duration)

    kept_duration = sum(end - start for start, end in keep)
    removed_duration = duration - kept_duration

    print_ranges("KEEP", keep, GREEN)

    print()
    print(c("EDIT SUMMARY", MAGENTA))
    print(f"  Original : {format_time(duration)}")
    print(f"  Removed  : {format_time(removed_duration)}")
    print(f"  Final    : {c(format_time(kept_duration), BOLD + GREEN)}")
    print()

    # ─────────────────────────────────────────────────────────────────────────
    # Output path
    # ─────────────────────────────────────────────────────────────────────────

    if args.output:
        output = Path(args.output).expanduser().resolve()
    else:
        output = video.with_name(
            video.stem + "_cut" + video.suffix
        )

    if output.exists():
        print(
            f"{c('WARNING', YELLOW)} Output already exists:\n"
            f"  {output}"
        )

        answer = input("Overwrite it? [y/N] ").strip().lower()

        if answer != "y":
            print("Cancelled.")
            sys.exit(0)

        output.unlink()

    # ─────────────────────────────────────────────────────────────────────────
    # Temporary workspace
    #
    # IMPORTANT:
    # Do NOT use /tmp.
    #
    # Temporary clips can be large, so keep them on the same filesystem
    # as the source unless the user explicitly provides another location.
    # ─────────────────────────────────────────────────────────────────────────

    work_parent = video.parent

    work_dir = Path(
        tempfile.mkdtemp(
            prefix=f".{video.stem}_cut_",
            dir=work_parent
        )
    )

    print(f"{c('WORKSPACE', DIM)}")
    print(f"  {work_dir}")

    clips = []

    try:

        # ─────────────────────────────────────────────────────────────────────
        # Extract KEEP ranges directly from ORIGINAL source
        # ─────────────────────────────────────────────────────────────────────

        print()
        print(c("EXTRACTING KEEP RANGES", BOLD + GREEN))
        print(
            f"{DIM}"
            "Each clip is extracted from the ORIGINAL source."
            f"{RESET}"
        )

        for index, (start, end) in enumerate(keep, 1):

            clip = work_dir / f"clip_{index:03d}{video.suffix}"

            clip_duration = end - start

            command = [
                "ffmpeg",
                "-hide_banner",
                "-loglevel", "warning",

                # Seek directly in the ORIGINAL file.
                "-ss", str(start),

                "-i", str(video),

                # Copy exactly the compressed streams.
                "-t", str(clip_duration),

                "-map", "0",
                "-c", "copy",

                # Give each temporary clip a clean timestamp origin.
                "-avoid_negative_ts", "make_zero",

                "-y",
                str(clip),
            ]

            run_ffmpeg(
                command,
                f"Extracting clip {index}/{len(keep)} "
                f"({format_time(start)} → {format_time(end)})"
            )

            clips.append(clip)

        # ─────────────────────────────────────────────────────────────────────
        # Build concat manifest
        # ─────────────────────────────────────────────────────────────────────

        concat_file = work_dir / "concat.txt"

        with open(concat_file, "w", encoding="utf-8") as f:
            for clip in clips:
                # concat demuxer uses single-quoted filenames.
                # Escape apostrophes safely.
                filename = str(clip).replace("'", "'\\''")
                f.write(f"file '{filename}'\n")

        # ─────────────────────────────────────────────────────────────────────
        # Concatenate WITHOUT RE-ENCODING
        # ─────────────────────────────────────────────────────────────────────

        concat_command = [
            "ffmpeg",
            "-hide_banner",
            "-loglevel", "warning",

            "-f", "concat",
            "-safe", "0",
            "-i", str(concat_file),

            "-map", "0",
            "-c", "copy",

            "-y",
            str(output),
        ]

        run_ffmpeg(
            concat_command,
            "Concatenating kept clips"
        )

        # ─────────────────────────────────────────────────────────────────────
        # Verify output
        # ─────────────────────────────────────────────────────────────────────

        output_duration = get_duration(output)

        print()
        print(c("=" * 64, GREEN))
        print(c(" COMPLETE ", BOLD + GREEN))
        print(c("=" * 64, GREEN))

        print(f"\n  Output   : {output}")
        print(f"  Duration : {format_time(output_duration)}")

        difference = abs(output_duration - kept_duration)

        if difference <= 2:
            print(
                f"  Status   : {c('✓ Duration looks correct', GREEN)}"
            )
        else:
            print(
                f"  Status   : {c('⚠ Duration differs from expectation', YELLOW)}"
            )
            print(
                f"  Expected : {format_time(kept_duration)}"
            )

    finally:

        # ─────────────────────────────────────────────────────────────────────
        # Cleanup
        # ─────────────────────────────────────────────────────────────────────

        if args.keep_temp:
            print()
            print(
                f"{c('TEMP FILES KEPT', YELLOW)}\n"
                f"  {work_dir}"
            )
        else:
            shutil.rmtree(work_dir, ignore_errors=True)

            print()
            print(
                f"{c('CLEANUP', DIM)} "
                f"Temporary clips removed."
            )


if __name__ == "__main__":
    main()
