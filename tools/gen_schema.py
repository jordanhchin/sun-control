#!/usr/bin/env python3
"""Generate sun-control's settings schema from an installed Sunshine web UI.

Usage: python3 -I tools/gen_schema.py /usr/share/sunshine/web > src/schema/sunshine.json

The web UI's Config page defines every setting with its tab and default; the
English locale supplies labels and descriptions; the compiled templates give
drop-down options and which platforms a setting applies to. Everything here is
best-effort: anything it can't work out falls back to a plain text field, and
sun-control shows settings missing from the schema in an "Other" tab anyway.
"""

import glob
import json
import os
import re
import sys


def js_object_to_json(src: str) -> str:
    """Convert the minified JS object literal used for tab defaults into JSON."""
    out = re.sub(r"`([^`]*)`", lambda m: json.dumps(m.group(1)), src)
    out = re.sub(r"!0\b", "true", out)
    out = re.sub(r"!1\b", "false", out)
    out = re.sub(r"([{,])\s*([A-Za-z_$][\w$]*)\s*:", r'\1"\2":', out)
    return out


def balanced(src: str, start: int, open_ch: str = "[", close_ch: str = "]") -> str:
    depth = 0
    for i in range(start, len(src)):
        c = src[i]
        if c == "`":  # skip template strings
            j = src.index("`", i + 1)
            i = j
            continue
        if c == open_ch:
            depth += 1
        elif c == close_ch:
            depth -= 1
            if depth == 0:
                return src[start : i + 1]
    raise ValueError("unbalanced")


def load_locale(web: str) -> dict:
    with open(os.path.join(web, "assets", "locale", "en.json"), encoding="utf-8") as f:
        data = json.load(f)
    flat = {}

    def walk(o, prefix=""):
        for k, v in o.items():
            if isinstance(v, dict):
                walk(v, f"{prefix}{k}.")
            else:
                flat[f"{prefix}{k}"] = v

    walk(data)
    return flat


def main(web: str) -> None:
    [config_js] = glob.glob(os.path.join(web, "assets", "Config-*.js"))
    with open(config_js, encoding="utf-8") as f:
        js = f.read()
    locale = load_locale(web)

    start = js.index("{id:`general`")
    tabs_src = balanced(js, js.rindex("[", 0, start))
    tabs = json.loads(js_object_to_json(tabs_src))

    # Platform filtering of whole tabs, mirroring the web UI's Config page.
    tab_platforms = {
        "nv": ["windows", "linux", "freebsd"],
        "amd": ["windows"],
        "qsv": ["windows"],
        "vt": ["macos"],
        "vaapi": ["linux", "freebsd"],
        "vulkan": ["linux", "freebsd"],
    }

    # Option variables like `xe={value:`0`}`.
    option_vars = dict(re.findall(r"\b([\w$]+)=\{(?:[^{}]*?,)?value:`([^`]*)`", js))

    def text(key: str) -> str | None:
        return locale.get(key)

    def options_for(key: str):
        m = re.search(r"h\(`select`,\{id:`" + re.escape(key) + r"`[^}]*\},\[", js)
        if not m:
            return None
        body = balanced(js, m.end() - 1)
        opts = []
        # Options built from variables, with a label expression.
        for om in re.finditer(r"\(`option`,([\w$]+),((?:`[^`]*`\+)?)f\([\w$]+\.\$t\(`([^`]+)`\)\)", body):
            var, prefix, label_key = om.groups()
            if var in option_vars:
                label = (prefix.strip("`+").strip() + " " + (text(label_key) or label_key)).strip()
                opts.append((om.start(), {"value": option_vars[var], "label": label}))
        # Literal label options: h(`option`,Var,`P1 `+...) handled above; plain literal labels:
        for om in re.finditer(r"\(`option`,([\w$]+),`([^`]*)`\s*[,)]", body):
            var, label = om.groups()
            if var in option_vars:
                opts.append((om.start(), {"value": option_vars[var], "label": label.strip()}))
        # Static HTML options.
        for om in re.finditer(r'<option value="([^"]*)">([^<]*)</option>', body):
            opts.append((om.start(), {"value": om.group(1), "label": om.group(2).strip()}))
        # Keep the order the web UI shows them in.
        seen, result = set(), []
        for _, o in sorted(opts, key=lambda p: p[0]):
            if o["value"] not in seen and o["value"] != "":
                seen.add(o["value"])
                result.append(o)
        return result or None

    # Platform slots of the web UI's PlatformLayout component:
    # `{windows:o(()=>[ ...fields... ]),linux:o(()=>[...])}`.
    slots = []
    for m in re.finditer(r"\b(windows|linux|macos|freebsd):[\w$]+\(\(\)=>\[", js):
        try:
            span = balanced(js, m.end() - 1)
        except ValueError:
            continue
        slots.append((m.end() - 1, m.end() - 1 + len(span), m.group(1)))
    all_platforms = ["windows", "linux", "macos", "freebsd"]

    def field_platforms(key: str):
        m = re.search(r"(?:h\(`label`,[\w$]+,f\([\w$]+\.\$t\(`config\." + re.escape(key) + r"`\)\)|id:`" + re.escape(key) + r"`)", js)
        if not m:
            return None
        # Inside a platform slot: only that platform.
        inside = [(start, name) for start, end, name in slots if start < m.start() < end]
        if inside:
            return [max(inside)[1]]
        before = js[max(0, m.start() - 500) : m.start()]
        # `e.platform===`x`||...?(d(),_(`div`,…,[` wrapping the field.
        cond = re.search(r"((?:[\w$]+\.platform===`\w+`(?:\|\|)?)+)\?\(d\(\),_\(`div`,[\w$]+,\[$", before[-160:])
        if cond:
            return re.findall(r"platform===`(\w+)`", cond.group(1))
        # `e.platform===`macos`?b(``,!0):(…)` hides the field on that platform.
        hidden = re.search(r"platform===`(\w+)`\?[\w$]+\(``,!0\):\(d\(\),_\(`div`,[\w$]+,\[[^\[]*$", before)
        if hidden:
            return [p for p in all_platforms if p != hidden.group(1)]
        return None

    out_tabs = []
    for tab in tabs:
        fields = []
        for key, default in tab["options"].items():
            options = options_for(key)
            if isinstance(default, (dict, list)):
                kind = "json"
            elif default in ("enabled", "disabled"):
                kind = "toggle"
            elif options:
                kind = "enum"
            elif isinstance(default, (int, float)) and not isinstance(default, bool):
                kind = "number"
            else:
                kind = "text"
            field = {
                "key": key,
                "label": text(f"config.{key}") or key.replace("_", " ").capitalize(),
                "kind": kind,
                "default": default,
            }
            desc = text(f"config.{key}_desc")
            if desc:
                field["desc"] = re.sub(r"<[^>]+>", "", desc)
            if kind == "enum":
                field["options"] = options
            plats = field_platforms(key)
            if plats:
                field["platforms"] = plats
            fields.append(field)
        entry = {"id": tab["id"], "name": text(tab["nameKey"]) or tab["id"], "fields": fields}
        if tab["id"] in tab_platforms:
            entry["platforms"] = tab_platforms[tab["id"]]
        out_tabs.append(entry)

    json.dump({"generated_from": os.path.basename(config_js), "tabs": out_tabs}, sys.stdout, indent=1, ensure_ascii=False)
    sys.stdout.write("\n")


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else "/usr/share/sunshine/web")
