"""TONE3000 (tone3000.com): pick a NAM tone in the browser, download it, load it.

Flow (OAuth 2.0 + PKCE, no client secret, nothing to host):
  1. A one-shot HTTP listener on 127.0.0.1:<random port>/callback (loopback
     redirect URIs never need registering with TONE3000).
  2. The browser opens TONE3000's own tone picker (prompt=select_tone, NAM only).
     The user is logged in there, browses, picks a tone.
  3. TONE3000 redirects to our listener with ?code&state&tone_id.
  4. Exchange the code for an access token, list the tone's models, let the
     user choose one, download it into the local model library.

Needs a publishable key (t3k_pub_...) from tone3000.com -> Settings -> API Keys.
Free tier covers exactly this (OAuth prompts + bounded lists).
"""
from __future__ import annotations

import base64
import hashlib
import http.server
import json
import re
import secrets
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
import webbrowser
from pathlib import Path

API = "https://www.tone3000.com/api/v1"
SIZE_ORDER = {"standard": 0, "lite": 1, "feather": 2, "nano": 3, "custom": 4}
PICK_TIMEOUT = 600  # seconds to pick a tone in the browser


class _NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args, **kwargs):
        return None  # we follow redirects ourselves, see fetch()


_opener = urllib.request.build_opener(_NoRedirect)


def fetch(url, data=None, headers=None, method=None, timeout=30) -> bytes:
    """GET/POST with manual redirects: the bearer token is only ever sent to
    the host it was issued for, never to the storage server a download may
    bounce to (signed storage URLs reject extra auth, and it shouldn't leak)."""
    headers = dict(headers or {})
    origin = urllib.parse.urlsplit(url).netloc
    for _ in range(6):
        req = urllib.request.Request(url, data=data, headers=headers, method=method)
        try:
            with _opener.open(req, timeout=timeout) as r:
                return r.read()
        except urllib.error.HTTPError as e:
            if e.code in (301, 302, 303, 307, 308) and e.headers.get("Location"):
                url = urllib.parse.urljoin(url, e.headers["Location"])
                if urllib.parse.urlsplit(url).netloc != origin:
                    headers.pop("Authorization", None)
                if e.code == 303:
                    data, method = None, "GET"
                continue
            body = e.read().decode("utf-8", "replace")[:300]
            raise RuntimeError(f"TONE3000 said {e.code}: {body or e.reason}") from None
        except urllib.error.URLError as e:
            raise RuntimeError(f"couldn't reach TONE3000 ({e.reason})") from None
    raise RuntimeError("too many redirects")


def _slug(s: str) -> str:
    return re.sub(r"[^A-Za-z0-9]+", "-", s).strip("-")[:60] or "model"


class Tone3000:
    """One pick-and-download flow at a time. The UI polls `status`."""

    def __init__(self, models_dir: Path, get_key, api: str = API, open_browser=webbrowser.open):
        self.models_dir = Path(models_dir)
        self.models_dir.mkdir(parents=True, exist_ok=True)
        self._get_key = get_key
        self._api = api.rstrip("/")
        self._open_browser = open_browser
        self._lock = threading.Lock()
        self._cancel = threading.Event()
        self._token = None
        self._tone = None
        self._models = []
        self.status = {"stage": "idle"}

    # --- the pick flow -------------------------------------------------------

    def start_pick(self):
        with self._lock:
            if self.status["stage"] in ("waiting", "loading", "downloading"):
                return self.status
            if not self._get_key():
                self.status = {"stage": "needs_key"}
                return self.status
            self._cancel.clear()
            self.status = {"stage": "waiting", "message": "Pick a tone in your browser…"}
        threading.Thread(target=self._pick, daemon=True).start()
        return self.status

    def cancel(self):
        self._cancel.set()
        self.status = {"stage": "idle"}

    def _set(self, **status):
        if not self._cancel.is_set():
            self.status = status

    def _pick(self):
        try:
            got = self._authorize()
            if got is None:
                return
            code, tone_id, verifier, redirect = got
            self._set(stage="loading", message="Getting the tone's models…")
            tok = json.loads(fetch(f"{self._api}/oauth/token", method="POST",
                                   headers={"Content-Type": "application/x-www-form-urlencoded"},
                                   data=urllib.parse.urlencode({
                                       "grant_type": "authorization_code", "code": code,
                                       "code_verifier": verifier, "redirect_uri": redirect,
                                       "client_id": self._get_key(),
                                   }).encode()))
            self._token = tok["access_token"]
            auth = {"Authorization": f"Bearer {self._token}"}
            tone = json.loads(fetch(f"{self._api}/tones/{tone_id}", headers=auth))
            page = json.loads(fetch(f"{self._api}/models?" + urllib.parse.urlencode(
                {"tone_id": tone_id, "page_size": 100}), headers=auth))
            models = [m for m in page.get("data", []) if m.get("model_url")]
            # NAM models carry an architecture version; IRs (format "ir") don't
            models = [m for m in models if m.get("architecture_version") is not None] or models
            models.sort(key=lambda m: (SIZE_ORDER.get(m.get("size") or "custom", 9), m.get("name") or ""))
            if not models:
                raise RuntimeError("that tone has no NAM models to download")
            self._tone, self._models = tone, models
            self._set(stage="choose", tone={
                "id": tone.get("id"), "title": tone.get("title") or f"Tone {tone_id}",
                "author": (tone.get("user") or {}).get("username"), "gear": tone.get("gear"),
                "url": tone.get("url"), "license": tone.get("license"),
            }, models=[{"id": m["id"], "name": m.get("name") or "Model", "size": m.get("size"),
                        "architecture": m.get("architecture_version")} for m in models])
        except Exception as e:  # shown to the user as-is
            self._set(stage="error", message=str(e))

    def _authorize(self):
        verifier = secrets.token_urlsafe(64)
        challenge = base64.urlsafe_b64encode(hashlib.sha256(verifier.encode()).digest()).rstrip(b"=").decode()
        state = secrets.token_urlsafe(16)
        result = {}
        done = threading.Event()

        class Callback(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                url = urllib.parse.urlsplit(self.path)
                if url.path != "/callback":
                    self.send_error(404)
                    return
                q = dict(urllib.parse.parse_qsl(url.query))
                result.update(q)
                ok = "code" in q and q.get("state") == state
                self.send_response(200)
                self.send_header("Content-Type", "text/html; charset=utf-8")
                self.end_headers()
                msg = ("Got it! Your tone is loading in Duet. You can close this tab."
                       if ok else "Something went wrong. Go back to Duet and try again.")
                self.wfile.write(_DONE_PAGE.replace("{msg}", msg).encode())
                done.set()

            def log_message(self, *args):
                pass

        server = http.server.HTTPServer(("127.0.0.1", 0), Callback)
        server.timeout = 0.5
        redirect = f"http://127.0.0.1:{server.server_port}/callback"
        url = f"{self._api}/oauth/authorize?" + urllib.parse.urlencode({
            "client_id": self._get_key(), "redirect_uri": redirect, "response_type": "code",
            "code_challenge": challenge, "code_challenge_method": "S256", "state": state,
            "prompt": "select_tone", "format": "nam",
        })
        self._open_browser(url)
        deadline = time.monotonic() + PICK_TIMEOUT
        try:
            while not done.is_set():
                if self._cancel.is_set():
                    return None
                if time.monotonic() > deadline:
                    raise RuntimeError("no tone picked in time; try again")
                server.handle_request()
        finally:
            server.server_close()
        if result.get("error"):
            raise RuntimeError("cancelled in the browser" if result["error"] == "access_denied"
                               else f"TONE3000: {result['error']}")
        if result.get("state") != state:
            raise RuntimeError("the browser answered for a different request; try again")
        if not result.get("tone_id"):
            raise RuntimeError("no tone was picked")
        return result["code"], result["tone_id"], verifier, redirect

    # --- download ------------------------------------------------------------

    def download(self, model_id) -> dict:
        model = next((m for m in self._models if str(m["id"]) == str(model_id)), None)
        if model is None or not self._token:
            raise RuntimeError("pick a tone first")
        self.status = {"stage": "downloading", "message": f"Downloading {model.get('name')}…"}
        try:
            data = fetch(model["model_url"], headers={"Authorization": f"Bearer {self._token}"}, timeout=120)
            json.loads(data)  # a .nam file is JSON: catch HTML error pages before saving them
        except Exception as e:
            self.status = {"stage": "error", "message": str(e)}
            raise
        title = self._tone.get("title") or "tone"
        path = self.models_dir / f"{_slug(title)}--{_slug(model.get('name') or '')}--{model['id']}.nam"
        path.write_bytes(data)
        self._remember(path, title, model.get("name"), model.get("size"), self._tone)
        self.status = {"stage": "done"}
        return {"path": str(path), "title": title, "name": model.get("name")}

    # --- local library ---------------------------------------------------------

    def _index_path(self):
        return self.models_dir / "library.json"

    def _index(self) -> dict:
        try:
            return json.loads(self._index_path().read_text(encoding="utf-8"))
        except (OSError, ValueError):
            return {}

    def _remember(self, path, title, name, size, tone):
        idx = self._index()
        idx[path.name] = {"title": title, "name": name, "size": size,
                          "author": ((tone or {}).get("user") or {}).get("username"),
                          "url": (tone or {}).get("url"), "added": time.time()}
        self._index_path().write_text(json.dumps(idx, indent=1), encoding="utf-8")

    def remember_file(self, path: str):
        """A .nam opened from disk joins the library too (by reference)."""
        idx = self._index()
        p = Path(path)
        if p.parent == self.models_dir or str(p) in idx:
            return
        idx[str(p)] = {"title": p.stem, "name": None, "size": None, "added": time.time()}
        self._index_path().write_text(json.dumps(idx, indent=1), encoding="utf-8")

    def library(self) -> list:
        idx = self._index()
        out = []
        for key, meta in idx.items():
            p = Path(key) if Path(key).is_absolute() else self.models_dir / key
            if p.exists():
                out.append({"path": str(p), **meta})
        for p in self.models_dir.glob("*.nam"):  # dropped in by hand
            if p.name not in idx:
                out.append({"path": str(p), "title": p.stem, "name": None, "size": None, "added": p.stat().st_mtime})
        out.sort(key=lambda m: -(m.get("added") or 0))
        return out


_DONE_PAGE = """<!doctype html><meta charset="utf-8"><title>Duet</title>
<style>body{margin:0;height:100vh;display:grid;place-items:center;background:#0e0d12;color:#efeaf4;
font:16px system-ui,sans-serif}div{text-align:center;max-width:420px;line-height:1.5}
b{display:block;font-size:22px;margin-bottom:8px;background:linear-gradient(90deg,#ff9548,#4fd6c6);
-webkit-background-clip:text;background-clip:text;color:transparent}</style>
<div><b>Duet</b>{msg}</div><script>setTimeout(()=>window.close(),1500)</script>"""
