"""Owner-behavior check for a Chromium browser's profile caches. Uses a scratch
--user-data-dir under OWNER_CHECK_DIR, so the real profile is never touched.
Needs network access. Usage: browser.py <browser exe>"""
import os, random, shutil, subprocess, sys, tempfile, time

exe = sys.argv[1]
root = os.environ.get("OWNER_CHECK_DIR") or os.path.join(tempfile.gettempdir(), "cleaner-owner-checks")
base = os.path.join(root, "browser-" + os.path.splitext(os.path.basename(exe))[0])
marker = os.path.join(base, ".cleaner-owner-check")
# Refuse to delete an existing folder that an owner check did not create.
if os.path.exists(base) and not os.path.exists(marker):
    sys.exit(f"refusing to delete {base}: it was not created by an owner check")
shutil.rmtree(base, ignore_errors=True)
os.makedirs(base)
open(marker, "w").close()
data = os.path.join(base, "User Data")
caches = [os.path.join(data, "Default", c) for c in ("Cache", "Code Cache", "GPUCache")]
pages = ["https://example.com", "https://www.wikipedia.org", "https://developer.mozilla.org/en-US/"]
common = [exe, "--headless=new", f"--user-data-dir={data}", "--no-first-run",
          "--no-default-browser-check", "--disable-sync"]

def load(url):
    """Loads `url` headless; success means the browser rendered a screenshot."""
    shot = os.path.join(base, "shot.png")
    if os.path.exists(shot):
        os.remove(shot)
    out = subprocess.run(common + ["--virtual-time-budget=5000", f"--screenshot={shot}", url],
                         capture_output=True, timeout=120)
    return out.returncode == 0 and os.path.exists(shot) and os.path.getsize(shot) > 1000

def files():
    return sum(len(f) for c in caches for _, _, f in os.walk(c))

def delete(fraction, rng):
    deleted = rejected = 0
    for cache in caches:
        for dirpath, _, names in os.walk(cache, topdown=False):
            for name in names:
                if rng.random() < fraction:
                    try:
                        os.remove(os.path.join(dirpath, name)); deleted += 1
                    except OSError:
                        rejected += 1
    return deleted, rejected

rng = random.Random(1)
for url in pages:
    assert load(url), f"populate {url}"
sentinel = os.path.join(data, "Default", "Preferences")
print(f"populated: {files()} cache files; caches present: {[os.path.isdir(c) for c in caches]}")

# Concurrent use: delete every cache file while a browser holds the profile open.
live = subprocess.Popen(common + ["--remote-debugging-port=0", pages[1]],
                        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
time.sleep(6)
print("while running: deleted %d, rejected %d" % delete(1.0, rng))
time.sleep(2)
print(f"browser still running after deletion: {live.poll() is None}")
live.terminate(); live.wait(30)
time.sleep(2)
ok = all(load(u) for u in pages)
print(f"after concurrent deletion: pages load {ok}, cache files rebuilt {files()}")

# Interruption: half the files removed while closed.
print("while closed: deleted %d, rejected %d" % delete(0.5, rng))
ok = all(load(u) for u in pages)
print(f"after partial deletion: pages load {ok}, cache files {files()}")

# Complete Clean: every cache file removed, cache folders kept.
delete(1.0, rng)
ok = all(load(u) for u in pages)
print(f"after full deletion: pages load {ok}, cache files {files()}")
print(f"Preferences present: {os.path.exists(sentinel)}")
