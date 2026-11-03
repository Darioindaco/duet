"use strict";
// Duet: updating itself. A while after it starts (and every few hours) Duet asks GitHub for a newer
// build; when there is one a card says so: Update downloads it in the background, then Restart
// puts it in place and starts the new version (updater.py). The song menu has "Check for updates".

const upd = { info: null, timer: null, dismissed: null };

function updSetup() {
  // (Preferences: "Check for updates by itself" off: only the song menu's Check for updates)
  const auto = async () => { try { if ((await api.prefs_get()).auto_update === false) return; } catch {} updCheck(false); };
  setTimeout(auto, 6000);
  setInterval(auto, 6 * 3600 * 1000);
}

async function updCheck(asked) {
  if (!api.update_check) return;
  let r;
  try { r = await api.update_check(); } catch (e) { r = { error: String(e) }; }
  upd.info = r;
  if (!asked) {
    if (r.available && upd.dismissed !== r.latest) updCard();
    return;
  }
  if (!r.supported) toast(r.why || "This copy doesn't update itself.");
  else if (r.error) toast(r.error, true);
  else if (r.available) updCard();
  else toast(`Duet is up to date (build ${r.current}).`);
}

function updCard() {
  const r = upd.info;
  let el = $("updCard");
  if (!el) {
    el = document.createElement("div");
    el.id = "updCard";
    el.className = "updcard";
    document.body.append(el);
  }
  const mb = r.size ? ` · ${(r.size / 1e6).toFixed(0)} MB` : "";
  el.innerHTML = `<div class="updtop"><b>A new Duet is ready</b><button class="qx" id="updX" title="Later">✕</button></div>
    <div class="updtxt">Build ${r.latest} (you have ${r.current})${mb}. Your songs and settings stay.</div>
    <div class="updbar" hidden><i></i></div>
    <div class="updbtns"><button class="mb" id="updLater">Later</button><button class="mb primary" id="updGo">Update</button></div>`;
  el.hidden = false;
  const close = () => { el.hidden = true; upd.dismissed = r.latest; };
  $("updX").onclick = close;
  $("updLater").onclick = close;
  $("updGo").onclick = updDownload;
}

async function updDownload() {
  const el = $("updCard"), go = $("updGo"), bar = el.querySelector(".updbar"), txt = el.querySelector(".updtxt");
  const r = await api.update_download();
  if (r.error) { toast(r.error, true); return; }
  go.disabled = true; go.textContent = "Downloading…"; bar.hidden = false;
  clearInterval(upd.timer);
  upd.timer = setInterval(async () => {
    const s = await api.update_status();
    bar.firstElementChild.style.width = (s.total ? Math.min(100, (100 * s.done) / s.total) : 5) + "%";
    if (s.state === "error") {
      clearInterval(upd.timer);
      toast(s.error, true);
      go.disabled = false; go.textContent = "Try again"; bar.hidden = true;
    } else if (s.state === "ready") {
      clearInterval(upd.timer);
      bar.hidden = true;
      txt.textContent = `Build ${upd.info.latest} is downloaded. Duet closes, puts it in place and opens again (your song is saved).`;
      go.disabled = false; go.textContent = "Restart now";
      go.onclick = updInstall;
    }
  }, 300);
}

async function updInstall() {
  if (song.st && song.st.recording) { toast("Stop recording first.", true); return; }
  const go = $("updGo");
  go.disabled = true; go.textContent = "Restarting…";
  const r = await api.update_install();
  if (r.error) { toast(r.error, true); go.disabled = false; go.textContent = "Restart now"; }
}
