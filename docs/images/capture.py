import json
import os
import pty
import re
import select
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import threading
import time
import fcntl
import termios
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlsplit, parse_qs

import pyte

HERE = Path(__file__).resolve().parent
SRC_PORT = 3000
GH_PORT = 3001
TOKEN = "demo-token-0123456789"

SOURCE = [
    ("alice", "site", False, "Personal site, built with Zola", "https://alice.example.com", ["website", "zola"], False),
    ("alice", "dotfiles", True, "Shell and editor configuration", "", [], False),
    ("alice", "talks", False, "Slides and demo code from my talks", "", ["slides"], False),
    ("alice", "notes", True, "", "", [], False),
    ("my-org", "api", False, "HTTP API for the billing service", "", ["rust", "api"], False),
    ("my-org", "web", False, "Customer dashboard", "", ["typescript"], False),
    ("my-org", "infra", True, "Provisioning for the build machines", "", [], False),
    ("my-org", "handbook", False, "Team handbook, moved to the wiki", "", [], True),
]

LATER = [
    ("my-org", "payments-poc", True, "Spike for the new payment provider", "", [], False),
    ("alice", "advent-2026", False, "Advent of Code 2026", "", [], False),
]


def git(*args, cwd=None, env=None):
    subprocess.run(["git", *args], cwd=cwd, env=env, check=True, capture_output=True)


class World:
    def __init__(self, root):
        self.root = Path(root)
        self.src_root = self.root / "forgejo"
        self.gh_root = self.root / "github"
        self.owners = {"alice": 1, "my-org": 2}
        self.repos = []
        self.gh = {}
        self.hooks = []
        self.lock = threading.Lock()

    def add_source(self, spec, rid):
        owner, name, private, desc, site, topics, archived = spec
        bare = self.src_root / owner / f"{name}.git"
        work = self.root / "work" / owner / name
        work.mkdir(parents=True)
        env = dict(os.environ, GIT_AUTHOR_NAME="Alice", GIT_AUTHOR_EMAIL="alice@example.com",
                   GIT_COMMITTER_NAME="Alice", GIT_COMMITTER_EMAIL="alice@example.com")
        git("init", "-q", "-b", "main", cwd=work)
        (work / "README.md").write_text(f"# {name}\n\n{desc}\n")
        git("add", ".", cwd=work)
        git("commit", "-q", "-m", "Initial commit", cwd=work, env=env)
        (work / "CHANGELOG.md").write_text("# Changelog\n")
        git("add", ".", cwd=work)
        git("commit", "-q", "-m", "Add a changelog", cwd=work, env=env)
        git("tag", "v0.1.0", cwd=work)
        git("branch", "next", cwd=work)
        bare.parent.mkdir(parents=True, exist_ok=True)
        git("clone", "-q", "--bare", str(work), str(bare))
        self.repos.append({
            "id": rid, "name": name, "full_name": f"{owner}/{name}",
            "owner": {"login": owner, "username": owner},
            "private": private, "description": desc, "website": site,
            "archived": archived, "default_branch": "main", "fork": False,
            "mirror": False, "empty": False, "topics": topics,
            "updated_at": "2026-11-13T09:00:00Z",
            "permissions": {"admin": True, "push": True, "pull": True},
        })

    def find(self, full):
        return next(r for r in self.repos if r["full_name"] == full)

    def change(self, full, **fields):
        with self.lock:
            r = self.find(full)
            if "name" in fields:
                old = self.src_root / f"{full}.git"
                old.rename(old.with_name(fields["name"] + ".git"))
            r.update(fields)
            r["updated_at"] = "2026-11-13T10:00:00Z"

    def remove(self, full):
        with self.lock:
            self.repos.remove(self.find(full))
            shutil.rmtree(self.src_root / f"{full}.git")

    def gh_create(self, owner, body):
        name = body["name"]
        bare = self.gh_root / owner / f"{name}.git"
        bare.parent.mkdir(parents=True, exist_ok=True)
        git("init", "-q", "--bare", str(bare))
        git("config", "http.receivepack", "true", cwd=bare)
        repo = {"name": name, "full_name": f"{owner}/{name}", "owner": {"login": owner},
                "private": True, "archived": False, "fork": False,
                "description": body.get("description"), "homepage": body.get("homepage"),
                "default_branch": "main", "topics": [],
                "html_url": f"https://github.com/{owner}/{name}", "pushed_at": ""}
        self.gh[(owner.lower(), name.lower())] = repo
        return repo


def cgi(handler, root, push):
    u = urlsplit(handler.path)
    length = int(handler.headers.get("Content-Length") or 0)
    body = b""
    if handler.headers.get("Transfer-Encoding", "").lower() == "chunked":
        chunks = []
        while True:
            size = int(handler.rfile.readline().strip(), 16)
            if size == 0:
                handler.rfile.readline()
                break
            chunks.append(handler.rfile.read(size))
            handler.rfile.readline()
        body = b"".join(chunks)
    elif length:
        body = handler.rfile.read(length)
    env = {
        "PATH": os.environ["PATH"],
        "GIT_PROJECT_ROOT": str(root),
        "GIT_HTTP_EXPORT_ALL": "1",
        "REQUEST_METHOD": handler.command,
        "PATH_INFO": u.path,
        "QUERY_STRING": u.query,
        "CONTENT_TYPE": handler.headers.get("Content-Type", ""),
        "CONTENT_LENGTH": str(len(body)),
        "REMOTE_ADDR": "127.0.0.1",
        "HTTP_CONTENT_ENCODING": handler.headers.get("Content-Encoding", ""),
        "GIT_PROTOCOL": handler.headers.get("Git-Protocol", ""),
    }
    if push:
        env["REMOTE_USER"] = "alice"
    out = subprocess.run(["git", "http-backend"], input=body, env=env, capture_output=True).stdout
    head, _, rest = out.partition(b"\r\n\r\n")
    status = 200
    headers = []
    for line in head.decode().split("\r\n"):
        k, _, v = line.partition(":")
        if k.lower() == "status":
            status = int(v.strip().split()[0])
        elif k:
            headers.append((k, v.strip()))
    handler.send_response(status)
    for k, v in headers:
        handler.send_header(k, v)
    handler.send_header("Content-Length", str(len(rest)))
    handler.end_headers()
    handler.wfile.write(rest)


class Base(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    world = None

    def log_message(self, *a):
        pass

    def reply(self, status, obj=None, headers=()):
        data = b"" if obj is None else json.dumps(obj).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        for k, v in headers:
            self.send_header(k, v)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def body(self):
        n = int(self.headers.get("Content-Length") or 0)
        return json.loads(self.rfile.read(n) or b"null")

    def do_GET(self):
        self.route()

    def do_POST(self):
        self.route()

    def do_PATCH(self):
        self.route()

    def do_PUT(self):
        self.route()

    def do_DELETE(self):
        self.route()


class Forgejo(Base):
    def route(self):
        w = self.world
        u = urlsplit(self.path)
        p = u.path
        if ".git/" in p:
            return cgi(self, w.src_root, False)
        if not p.startswith("/api/v1/"):
            return self.reply(404)
        p = p[len("/api/v1"):]
        q = parse_qs(u.query)
        with w.lock:
            if p == "/user":
                return self.reply(200, {"login": "alice", "is_admin": True})
            if p.startswith("/users/") and p.count("/") == 2:
                owner = p.split("/")[2]
                if owner in w.owners:
                    return self.reply(200, {"id": w.owners[owner], "login": owner})
                return self.reply(404)
            if p == "/repos/search":
                uid = int(q.get("uid", ["0"])[0])
                page = int(q.get("page", ["1"])[0])
                rows = [r for r in w.repos if w.owners[r["owner"]["login"]] == uid]
                rows = rows if page == 1 else []
                return self.reply(200, {"ok": True, "data": rows}, [("X-Total-Count", str(len(rows)))])
            if p.startswith("/repos/") and p.count("/") == 3:
                _, _, owner, name = p.split("/")
                for r in w.repos:
                    if r["full_name"].lower() == f"{owner}/{name}".lower():
                        return self.reply(200, r)
                return self.reply(404)
            if p == "/admin/hooks" and self.command == "GET":
                return self.reply(200, w.hooks)
            if p == "/admin/hooks" and self.command == "POST":
                b = self.body()
                hook = {"id": len(w.hooks) + 1, "type": b.get("type"), "active": True,
                        "config": {"url": b.get("config", {}).get("url", "")}}
                w.hooks.append(hook)
                return self.reply(201, hook)
            if p.startswith("/admin/hooks/"):
                hid = int(p.rsplit("/", 1)[1])
                for h in w.hooks:
                    if h["id"] == hid:
                        if self.command == "DELETE":
                            w.hooks.remove(h)
                            return self.reply(204)
                        return self.reply(200, h)
                return self.reply(404)
        return self.reply(404)


class GitHub(Base):
    def route(self):
        w = self.world
        u = urlsplit(self.path)
        p = u.path
        if ".git/" in p:
            return cgi(self, w.gh_root, True)
        if not p.startswith("/api/"):
            return self.reply(404)
        p = p[len("/api"):]
        m = self.command
        with w.lock:
            if p == "/user":
                return self.reply(200, {"login": "alice"}, [("X-OAuth-Scopes", "delete_repo, repo")])
            if p.startswith("/users/") and p.count("/") == 2:
                login = p.split("/")[2]
                kind = {"alice": "User", "my-org": "Organization"}.get(login)
                return self.reply(200, {"login": login, "type": kind}) if kind else self.reply(404)
            if p == "/search/issues":
                return self.reply(200, {"total_count": 0, "items": []})
            if p == "/user/repos" and m == "GET":
                return self.reply(200, [r for (o, _), r in w.gh.items() if o == "alice"])
            if p.startswith("/orgs/") and p.endswith("/repos") and m == "GET":
                org = p.split("/")[2].lower()
                return self.reply(200, [r for (o, _), r in w.gh.items() if o == org])
            if p == "/user/repos" and m == "POST":
                return self.reply(201, w.gh_create("alice", self.body()))
            if p.startswith("/orgs/") and p.endswith("/repos") and m == "POST":
                return self.reply(201, w.gh_create(p.split("/")[2], self.body()))
            parts = p.split("/")
            if p.startswith("/repos/") and len(parts) >= 4:
                key = (parts[2].lower(), parts[3].lower())
                repo = w.gh.get(key)
                if repo is None:
                    return self.reply(404, {"message": "Not Found"})
                if len(parts) == 5 and parts[4] == "topics" and m == "PUT":
                    repo["topics"] = self.body().get("names", [])
                    return self.reply(200, {"names": repo["topics"]})
                if len(parts) == 4 and m == "GET":
                    return self.reply(200, repo)
                if len(parts) == 4 and m == "PATCH":
                    b = self.body()
                    for k, v in b.items():
                        if k == "name":
                            del w.gh[key]
                            repo["name"] = v
                            w.gh[(key[0], v.lower())] = repo
                        else:
                            repo[k] = v
                    return self.reply(200, repo)
                if len(parts) == 4 and m == "DELETE":
                    del w.gh[key]
                    return self.reply(204)
        return self.reply(404, {"message": "Not Found"})


def serve(world):
    servers = []
    for port, cls in ((SRC_PORT, Forgejo), (GH_PORT, GitHub)):
        h = type(cls.__name__, (cls,), {"world": world})
        s = ThreadingHTTPServer(("127.0.0.1", port), h)
        threading.Thread(target=s.serve_forever, daemon=True).start()
        servers.append(s)
    return servers


CONFIG = """[source]
type = "forgejo"
url = "http://localhost:3000"
token_file = "source.token"

[github]
token_file = "github.token"
api_url = "http://localhost:3001/api"
git_url = "http://localhost:3001"

[accounts]
alice = "alice"
my-org = "my-org"

[listen]
secret = "{secret}"

[filter]
exclude = ["alice/notes"]

[paths]
state_dir = "state"
"""


def run_pty(argv, cwd, env, cols, rows, keys=(), settle=0.6, timeout=30):
    screen = pyte.Screen(cols, rows)
    stream = pyte.ByteStream(screen)
    pid, fd = pty.fork()
    if pid == 0:
        os.chdir(cwd)
        os.execvpe(argv[0], argv, env)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
    os.kill(pid, signal.SIGWINCH)
    keys = list(keys)
    deadline = time.time() + timeout
    last = time.time()
    snapshot = None
    while time.time() < deadline:
        r, _, _ = select.select([fd], [], [], 0.1)
        if r:
            try:
                data = os.read(fd, 65536)
            except OSError:
                break
            if not data:
                break
            stream.feed(data)
            last = time.time()
            continue
        if time.time() - last > settle:
            if not keys:
                if snapshot is not None:
                    break
                done, _ = os.waitpid(pid, os.WNOHANG)
                if done:
                    break
                continue
            k = keys.pop(0)
            if k == "SNAP":
                snapshot = copy_screen(screen)
            else:
                os.write(fd, k.encode())
            last = time.time()
    if snapshot is None:
        snapshot = copy_screen(screen)
    try:
        os.kill(pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    try:
        os.waitpid(pid, 0)
    except ChildProcessError:
        pass
    os.close(fd)
    return snapshot


def copy_screen(screen):
    rows = []
    for y in range(screen.lines):
        line = screen.buffer[y]
        rows.append([(line[x].data, line[x].fg, line[x].bg, line[x].bold, line[x].reverse)
                     for x in range(screen.columns)])
    return rows


PALETTE = {
    "default": "#d4d4d8", "black": "#3f3f46", "red": "#f87171", "green": "#4ade80",
    "brown": "#facc15", "yellow": "#facc15", "blue": "#60a5fa", "magenta": "#c084fc",
    "cyan": "#22d3ee", "white": "#e4e4e7", "brightblack": "#71717a", "brightred": "#fca5a5",
    "brightgreen": "#86efac", "brightyellow": "#fde68a", "brightblue": "#93c5fd",
    "brightmagenta": "#d8b4fe", "brightcyan": "#67e8f9", "brightwhite": "#fafafa",
}
BG = "#18181b"
FG = PALETTE["default"]


def color(c, default):
    if c == "default":
        return default
    if c in PALETTE:
        return PALETTE[c]
    if re.fullmatch(r"[0-9a-fA-F]{6}", c):
        return "#" + c.lower()
    return default


def esc(s):
    return s.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


def trim(rows):
    while rows and all(ch[0] == " " and ch[2] == "default" and not ch[4] for ch in rows[-1]):
        rows.pop()
    return rows


def render(blocks, path, title, cols, cw=8.4, lh=19, fs=14):
    lines = []
    for prompt, rows in blocks:
        if prompt is not None:
            lines.append([("$ ", PALETTE["green"], None, True)] + [(prompt, PALETTE["brightwhite"], None, True)])
        for row in rows:
            runs = []
            for data, fg, bg, bold, rev in row:
                f, b = color(fg, FG), (None if bg == "default" else color(bg, BG))
                if rev:
                    f, b = (b or BG), f
                if runs and runs[-1][1:] == (f, b, bold):
                    runs[-1] = (runs[-1][0] + data, f, b, bold)
                else:
                    runs.append((data, f, b, bold))
            lines.append(runs)
    pad, bar = 20, 36
    used = max((sum(len(r[0]) for r in runs) - (len(runs[-1][0]) - len(runs[-1][0].rstrip(" ")) if runs and runs[-1][2] is None else 0) for runs in lines), default=0)
    width = int(min(cols, used + 2) * cw + pad * 2)
    height = int(bar + pad + len(lines) * lh + pad - 4)
    out = [f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}" role="img" aria-label="{esc(title)}">',
           f"<title>{esc(title)}</title>",
           "<style>text{font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,'DejaVu Sans Mono','Liberation Mono',monospace;font-size:%dpx;white-space:pre}.b{font-weight:700}</style>" % fs,
           f'<rect width="{width}" height="{height}" rx="10" fill="{BG}"/>',
           f'<rect x="0.5" y="0.5" width="{width - 1}" height="{height - 1}" rx="10" fill="none" stroke="#3f3f46"/>',
           f'<path d="M0.5 {bar}H{width - 0.5}" stroke="#27272a"/>']
    for i, c in enumerate(("#52525b", "#52525b", "#52525b")):
        out.append(f'<circle cx="{pad + i * 18}" cy="{bar / 2}" r="5.5" fill="{c}"/>')
    out.append(f'<text x="{width / 2}" y="{bar / 2 + 4.5}" fill="#a1a1aa" text-anchor="middle" style="font-size:12.5px">{esc(title)}</text>')
    y0 = bar + pad + fs - 2
    for i, runs in enumerate(lines):
        y = y0 + i * lh
        col = 0
        for data, f, b, bold in runs:
            n = len(data)
            x = pad + col * cw
            if b:
                out.append(f'<rect x="{x:.1f}" y="{y - fs + 1:.1f}" width="{n * cw:.1f}" height="{lh}" fill="{b}"/>')
            cls = ' class="b"' if bold else ""
            for m in re.finditer(r"\S(?:\S| (?=\S))*", data):
                text = m.group(0)
                tx = x + m.start() * cw
                out.append(f'<text x="{tx:.1f}" y="{y}" fill="{f}"{cls} textLength="{len(text) * cw:.1f}" lengthAdjust="spacingAndGlyphs">{esc(text)}</text>')
            col += n
    out.append("</svg>")
    Path(path).write_text("\n".join(out) + "\n")


def main():
    binary = os.path.abspath(sys.argv[1])
    outdir = Path(sys.argv[2]) if len(sys.argv) > 2 else HERE
    base = Path(os.environ.get("XDG_CACHE_HOME", Path.home() / ".cache"))
    base.mkdir(parents=True, exist_ok=True)
    root = Path(tempfile.mkdtemp(prefix="gitsync-capture-", dir=base))
    try:
        bindir = root / "bin"
        bindir.mkdir()
        shutil.copy(binary, bindir / "gitsync")
        world = World(root)
        for i, spec in enumerate(SOURCE, start=1):
            world.add_source(spec, i)
        serve(world)
        demo = root / "demo"
        demo.mkdir()
        (demo / "config.toml").write_text(CONFIG.format(secret="s3cr3t-" + "x" * 24))
        (demo / "source.token").write_text(TOKEN)
        (demo / "github.token").write_text(TOKEN)
        os.chmod(demo / "config.toml", 0o600)
        env = {"PATH": f"{bindir}:/usr/local/bin:/usr/bin:/bin", "HOME": str(root),
               "TERM": "xterm-256color", "LANG": "C.UTF-8", "COLORTERM": "truecolor",
               "GIT_CONFIG_NOSYSTEM": "1"}
        cols = 140

        def cmd(line, rows=60, keys=(), cols=cols):
            return trim(run_pty(["gitsync", *line.split()], demo, env, cols, rows, keys))

        check = cmd("check")
        sync = cmd("sync")
        status = cmd("status")
        render([("gitsync check", check), ("gitsync sync", sync), ("gitsync status", status)],
               outdir / "terminal.svg", "gitsync", cols)
        cmd("config set filter.new_repos review")
        for i, spec in enumerate(LATER, start=len(SOURCE) + 1):
            world.add_source(spec, i)
        cmd("sync")
        world.change("my-org/web", name="dashboard", full_name="my-org/dashboard")
        world.change("alice/talks", archived=True)
        world.change("my-org/api", description="HTTP API for billing and invoices")
        world.remove("my-org/handbook")
        why = cmd("why my-org/payments-poc")
        plan = cmd("plan")
        render([("gitsync why my-org/payments-poc", why), ("gitsync plan", plan)],
               outdir / "plan.svg", "gitsync", cols)
        tui = trim(run_pty(["gitsync", "repos"], demo, env, 112, 18, keys=["f", "f", "f", " ", "SNAP"]))
        render([(None, tui)], outdir / "selector.svg", "gitsync repos", 112)
    finally:
        if os.environ.get("KEEP") != "1":
            shutil.rmtree(root, ignore_errors=True)
        else:
            print(root)


if __name__ == "__main__":
    main()
