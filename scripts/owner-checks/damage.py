"""Simulates a Clean with scattered rejected deletions: deletes a seeded random
fraction (default half) of the files under a cache folder, then prunes directories it emptied.
Usage: damage.py <cache dir> [fraction]"""
import os, random, sys

root = sys.argv[1]
fraction = float(sys.argv[2]) if len(sys.argv) > 2 else 0.5
rng = random.Random(1)
deleted = kept = 0
for dirpath, dirnames, filenames in os.walk(root, topdown=False):
    removed_here = False
    for name in sorted(filenames):
        path = os.path.join(dirpath, name)
        if rng.random() < fraction:
            try:
                os.remove(path)
                deleted += 1
                removed_here = True
            except OSError:
                kept += 1
        else:
            kept += 1
    if dirpath != root and removed_here and not os.listdir(dirpath):
        os.rmdir(dirpath)
print(f"deleted {deleted} files, kept {kept}")
