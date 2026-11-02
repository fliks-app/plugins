# Plugins a change can affect: its own files, a workspace crate it uses, or a Cargo.lock entry in
# its dependency closure. Tooling and root manifest changes (other than members) affect them all.
import json, os, subprocess, sys, tomllib

base, head = sys.argv[1], sys.argv[2]


def git(*args):
    return subprocess.run(["git", *args], capture_output=True, text=True, check=True).stdout


def toml_at(rev, path):
    try:
        return tomllib.loads(git("show", f"{rev}:{path}"))
    except subprocess.CalledProcessError:
        return {}


changed = git("diff", "--name-only", base, head).split()
root = toml_at(head, "Cargo.toml")
members = root["workspace"]["members"]
pkg_dir = {toml_at(head, f"{m}/Cargo.toml")["package"]["name"]: m for m in members}
plugins = sorted((d, n) for n, d in pkg_dir.items() if d.startswith("plugins/"))

old_root = toml_at(base, "Cargo.toml")
old_root.get("workspace", {}).pop("members", None)
root_rest = {**root, "workspace": {k: v for k, v in root["workspace"].items() if k != "members"}}
everything = old_root != root_rest or any(
    f.startswith((".cargo/", "xtask/", "rust-toolchain")) or f in (".github/workflows/ci.yml", ".github/scripts/affected.py")
    for f in changed
)


def lock(rev):
    return {(p["name"], p["version"]): p for p in toml_at(rev, "Cargo.lock").get("package", [])}


new_lock, old_lock = lock(head), lock(base)
dirty_lock = {k for k, p in new_lock.items() if old_lock.get(k) != p}
by_name = {}
for name, version in new_lock:
    by_name.setdefault(name, []).append(version)


def closure(name):
    seen, todo = set(), [(name, v) for v in by_name.get(name, [])]
    while todo:
        key = todo.pop()
        if key in seen:
            continue
        seen.add(key)
        for dep in new_lock[key].get("dependencies", []):
            parts = dep.split(" ")
            todo += [(parts[0], v) for v in by_name.get(parts[0], []) if len(parts) == 1 or v == parts[1]]
    return seen


def touched(name):
    return any(f.startswith(pkg_dir[name] + "/") for f in changed)


def affected(name):
    deps = closure(name)
    return bool(deps & dirty_lock) or any(n in pkg_dir and touched(n) for n, _ in deps)


out = [d for d, n in plugins if everything or affected(n)]
crates = everything or any(affected(n) for n, d in pkg_dir.items() if d.startswith("crates/"))
with open(os.environ.get("GITHUB_OUTPUT", "/dev/stdout"), "a") as f:
    f.write(f"plugins={json.dumps(out)}\ncrates={str(crates).lower()}\n")
