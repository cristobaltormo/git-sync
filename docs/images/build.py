import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
IMAGES = ROOT / "docs" / "images"
GITHUB = ROOT / ".github"

TAGLINE = ["Mirror your self-hosted git server to GitHub,", "and keep it in step."]
SERVERS = "Forgejo · Gitea · GitLab · Gogs · GitBucket · OneDev · Bitbucket  →  GitHub"
FACTS = "Rust · single static binary · ~4 MB RAM"

LIGHT = {
    "bg": "none", "box": "#f8fafc", "stroke": "#cbd5e1", "hot": "#f0fdfa", "hotstroke": "#0f766e",
    "title": "#0f172a", "sub": "#475569", "line": "#64748b", "label": "#334155", "accent": "#0f766e",
}
DARK = {
    "bg": "none", "box": "#161b22", "stroke": "#30363d", "hot": "#0d2a27", "hotstroke": "#2dd4bf",
    "title": "#e6edf3", "sub": "#9da7b3", "line": "#8b949e", "label": "#c9d1d9", "accent": "#2dd4bf",
}

BOXES = [
    ("src", 20, 168, 190, 124, False, "Git server", ["Forgejo, Gitea, GitLab,", "Gogs, GitBucket, OneDev,", "Bitbucket"]),
    ("hook", 290, 64, 196, 84, False, "Webhook listener", ["signed events only"]),
    ("poll", 290, 312, 196, 84, False, "Poller", ["lists repos every 5 s"]),
    ("queue", 566, 188, 160, 84, True, "Queue", ["one job per repo"]),
    ("work", 806, 188, 170, 84, True, "Workers", ["sync.workers, default 2"]),
    ("state", 806, 340, 170, 70, False, "state.json", ["and bare mirrors"]),
    ("gh", 1056, 168, 124, 124, False, "GitHub", ["repos and", "settings"]),
]

ARROWS = [
    ("M210 206 C250 206 250 106 286 106", "webhook", 242, 146, "end"),
    ("M210 256 C250 256 250 354 286 354", "listing", 242, 324, "end"),
    ("M486 106 C530 106 520 214 562 214", "", 0, 0, "start"),
    ("M486 354 C530 354 520 248 562 248", "", 0, 0, "start"),
    ("M726 230 H802", "", 0, 0, "start"),
    ("M976 214 H1052", "git push", 1014, 204, "middle"),
    ("M976 248 H1052", "REST API", 1014, 270, "middle"),
    ("M891 272 V336", "", 0, 0, "start"),
    ("M115 168 V36 H891 V184", "git fetch into a bare mirror", 503, 26, "middle"),
]


def architecture(p):
    font = "-apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif"
    out = [
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1200 430" role="img" aria-labelledby="t d">',
        '<title id="t">How gitsync works</title>',
        '<desc id="d">The git server sends webhooks to the listener and is polled for its repository listing. Both feed a queue with one job per repository. Workers take jobs, fetch from the git server into a bare mirror, push to GitHub and apply settings through the GitHub API. State is kept in state.json.</desc>',
        "<defs>",
        f'<marker id="a" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0L10 5L0 10z" fill="{p["line"]}"/></marker>',
        "<style>",
        f".box{{fill:{p['box']};stroke:{p['stroke']};stroke-width:1.5}}",
        f".hot{{fill:{p['hot']};stroke:{p['hotstroke']};stroke-width:1.8}}",
        f".t{{font:600 16px {font};fill:{p['title']};text-anchor:middle}}",
        f".s{{font:400 13px {font};fill:{p['sub']};text-anchor:middle}}",
        f".l{{font:400 12.5px {font};fill:{p['label']}}}",
        f".line{{stroke:{p['line']};stroke-width:1.6;fill:none;marker-end:url(#a)}}",
        "</style>",
        "</defs>",
    ]
    for _, x, y, w, h, hot, title, sub in BOXES:
        out.append(f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="12" class="{"hot" if hot else "box"}"/>')
        top = y + h / 2 - (len(sub) * 17) / 2 + 4
        out.append(f'<text x="{x + w / 2}" y="{top:.0f}" class="t">{title}</text>')
        for i, s in enumerate(sub):
            out.append(f'<text x="{x + w / 2}" y="{top + 22 + i * 17:.0f}" class="s">{s}</text>')
    for d, label, lx, ly, anchor in ARROWS:
        out.append(f'<path d="{d}" class="line"/>')
        if label:
            out.append(f'<text x="{lx}" y="{ly}" class="l" text-anchor="{anchor}">{label}</text>')
    out.append("</svg>")
    return "\n".join(out) + "\n"


def logo_inner():
    text = (IMAGES / "logo.svg").read_text()
    return re.sub(r"^<svg[^>]*>|</svg>\s*$", "", text.strip())


def social():
    font = "Lato,'Helvetica Neue',Arial,sans-serif"
    out = [
        '<svg xmlns="http://www.w3.org/2000/svg" width="1280" height="640" viewBox="0 0 1280 640">',
        "<defs>",
        '<linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#0b1416"/><stop offset="1" stop-color="#0f1f22"/></linearGradient>',
        "</defs>",
        '<rect width="1280" height="640" fill="url(#g)"/>',
        '<path d="M0 0H1280V640H0Z" fill="none" stroke="#1f3236" stroke-width="2"/>',
        f'<g transform="translate(112 150) scale(2.1875)">{logo_inner()}</g>',
        f'<text x="282" y="236" font-family="{font}" font-weight="700" font-size="104" fill="#f1f5f9" letter-spacing="-1">gitsync</text>',
    ]
    for i, line in enumerate(TAGLINE):
        out.append(f'<text x="112" y="{372 + i * 54}" font-family="{font}" font-weight="400" font-size="44" fill="#d5dee3">{line}</text>')
    out.append('<path d="M112 486H1168" stroke="#22383c" stroke-width="2"/>')
    out.append(f'<text x="112" y="536" font-family="{font}" font-size="27" fill="#5eead4">{SERVERS}</text>')
    out.append(f'<text x="112" y="580" font-family="{font}" font-size="27" fill="#94a3b8">{FACTS}</text>')
    out.append("</svg>")
    return "\n".join(out) + "\n"


def main():
    (IMAGES / "architecture.svg").write_text(architecture(LIGHT))
    (IMAGES / "architecture-dark.svg").write_text(architecture(DARK))
    svg = social()
    (GITHUB / "social-preview.svg").write_text(svg)
    if "--no-png" not in sys.argv:
        import cairosvg
        cairosvg.svg2png(bytestring=svg.encode(), write_to=str(GITHUB / "social-preview.png"),
                         output_width=1280, output_height=640)


if __name__ == "__main__":
    main()
