"""Simulates Clean stopping partway: deletes files in the core's traversal order
(NTFS upper-case name order, depth-first, emptied directories pruned) and stops
after `fraction` of all files. Usage: interrupt.py <cache dir> <fraction>"""
import os, sys

root, fraction = sys.argv[1], float(sys.argv[2])
total = sum(len(files) for _, _, files in os.walk(root))
budget = int(total * fraction)

def walk(path):
    """Returns True when the directory was emptied by this walk."""
    global budget
    removed = False
    for entry in sorted(os.scandir(path), key=lambda e: e.name.upper()):
        if budget <= 0:
            return False
        if entry.is_dir(follow_symlinks=False):
            if walk(entry.path) and budget > 0:
                os.rmdir(entry.path)
                removed = True
        else:
            os.remove(entry.path)
            budget -= 1
            removed = True
    return removed and not os.listdir(path)

walk(root)
print(f"interrupted after {int(total * fraction)} of {total} files")
