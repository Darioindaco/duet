// Songs as files you look after: Save (Ctrl+S), a New song window (name, where it's kept, tempo,
// time signature, key), names that don't clash, the Recent list, Preferences, and a question
// before Duet closes. (Duet still saves every change by itself: Save is the one you can see.)

const prefs = { v: null };
async function loadPrefs() {
  try { prefs.v = await api.prefs_get(); } catch { prefs.v = null; }
  return prefs.v || { confirm_close: true, song_location: "", new_bpm: 120, new_beats: 4, auto_update: true, mix_threads: true, plugin_sandbox: false, projects_dir: "" };
}
const shortPath = (p) => { const parts = String(p || "").split(/[\\/]/).filter(Boolean); return parts.length > 3 ? "…/" + parts.slice(-3).join("/") : p; };

// ---------- save ----------
async function saveSong() {
  const r = await api.daw_save();
  if (!r || r.error) { toast((r && r.error) || "Couldn't save.", true); return false; }
  if (song.view) { song.view.unsaved = false; paintSaved(); }
  toast(`Saved "${song.view?.meta?.name || "the song"}"`);
  return true;
}
// the song's name in the top bar has a dot while there are changes since the last Save
function paintSaved() {
  const v = song.view;
  if (!v) return;
  const nm = $("songName");
  nm.textContent = v.meta.name + (v.unsaved ? " •" : "");
  nm.parentElement.title = (v.unsaved ? "Changed since you last saved (Ctrl+S). It's kept on disk all the same. " : "Saved. ")
    + (v.shared ? "Open on both sides: you're working on it together." : "Only on this computer for now. Connect to work on it together.");
}

// ---------- names ----------
// null if `name` is fine for a song, else why not (another song here has it)
async function songNameProblem(name, but) {
  const n = String(name || "").trim();
  if (!n) return { msg: "Give it a name." };
  const r = await api.daw_name_free(n, but || null);
  return r.taken ? { msg: `There's a song called "${n}" already.`, suggest: r.suggest } : null;
}
async function renameSong(name) {
  const n = String(name || "").trim();
  if (!n || n === song.view.meta.name) return true;
  const bad = await songNameProblem(n, song.view.pid);
  if (bad) { toast(`${bad.msg} Try "${bad.suggest}".`, true); return false; }
  songEdit([["meta", "meta", { name: n }]]);
  return true;
}

// ---------- new song ----------
const ns = { location: "", start: "track" };
async function openNewSong() {
  const m = $("newSongM"), p = await loadPrefs();
  ns.location = p.song_location || "";
  ns.start = "track";
  const def = await api.daw_name_free("New song");
  $("nsName").value = def.suggest || "New song";
  $("nsMsg").textContent = "";
  $("nsBpm").value = String(+(+p.new_bpm || 120).toFixed(2));
  $("nsBeats").value = String(p.new_beats || 4);
  const ks = $("nsKey");
  if (!ks.options.length) {
    ks.append(new Option("None", ""));
    for (const k of (typeof KEYS !== "undefined" ? KEYS : [])) ks.append(new Option(typeof keyLabel === "function" ? keyLabel(k) : k, k));
  }
  ks.value = "";
  $("nsShared").hidden = !song.view?.shared;
  paintNewSong(p);
  m.hidden = false;
  setTimeout(() => { $("nsName").focus(); $("nsName").select(); }, 30);
}
function paintNewSong(p) {
  const dflt = (p || prefs.v || {}).projects_dir || "Duet's songs folder";
  $("nsPath").textContent = ns.location ? shortPath(ns.location) + "/" + ($("nsName").value.trim() || "…") : "Duet's songs folder";
  $("nsPath").title = ns.location || dflt;
  $("nsDefault").hidden = !ns.location;
  for (const b of $("nsStart").querySelectorAll("button")) b.classList.toggle("on", b.dataset.v === ns.start);
}
async function checkNewName() {
  const bad = await songNameProblem($("nsName").value);
  $("nsMsg").textContent = bad ? bad.msg + (bad.suggest ? ` "${bad.suggest}" is free.` : "") : "";
  $("nsMsg").classList.toggle("bad", !!bad);
  $("nsGo").disabled = !!bad;
  return !bad;
}
async function createNewSong() {
  if (!(await checkNewName())) { $("nsName").focus(); return; }
  const bpm = parseFloat(String($("nsBpm").value).replace(",", "."));
  const r = await api.daw_new({
    name: $("nsName").value.trim(), location: ns.location || null,
    bpm: isFinite(bpm) ? bpm : 120, beats: +$("nsBeats").value, key: $("nsKey").value || null, track: ns.start === "track",
  });
  if (r.error) { $("nsMsg").textContent = r.error; $("nsMsg").classList.add("bad"); return; }
  $("newSongM").hidden = true;
  setSongView(r.view);
  toast(`New song: ${r.view.meta.name}`);
}

// ---------- preferences ----------
async function openPrefs() {
  const p = await loadPrefs();
  $("pfLoc").textContent = p.song_location ? shortPath(p.song_location) : "Duet's songs folder";
  $("pfLoc").title = p.song_location || p.projects_dir;
  $("pfLocDefault").hidden = !p.song_location;
  $("pfBpm").value = String(+(+p.new_bpm || 120).toFixed(2));
  $("pfBeats").value = String(p.new_beats || 4);
  $("pfConfirm").checked = !!p.confirm_close;
  $("pfUpdate").checked = !!p.auto_update;
  $("pfThreads").checked = !!p.mix_threads;
  $("pfSandbox").checked = !!p.plugin_sandbox;
  $("prefsM").hidden = false;
}
async function setPref(key, value) {
  const r = await api.prefs_set(key, value);
  if (r && r.error) toast(r.error, true);
  if (prefs.v && r && r.ok) prefs.v[key] = r.value;
  return r;
}

// ---------- closing ----------
// Python asks this when the window's close button is pressed (if Preferences say to ask)
function askClose() {
  const v = song.view, m = $("closeM");
  const name = v ? `"${esc(v.meta.name)}"` : "the song";
  $("clText").innerHTML = v && v.unsaved
    ? `${name} has changes since you last saved. They're on disk already (Duet saves as you go), but <b>Save</b> also keeps this moment as a version called "Saved".`
    : `${name} is saved.`;
  $("clSave").textContent = v && v.unsaved ? "Save and close" : "Close";
  $("clClose").hidden = !(v && v.unsaved);
  for (const id of ["newSongM", "prefsM"]) $(id).hidden = true;
  m.hidden = false;
  setTimeout(() => $("clSave").focus(), 30);
}
window.askClose = askClose;

function projectSetup() {
  const closeOn = (id, x) => { $(x).onclick = () => ($(id).hidden = true); $(id).addEventListener("pointerdown", (e) => { if (e.target === $(id)) $(id).hidden = true; }); };
  // new song
  closeOn("newSongM", "nsX");
  $("nsCancel").onclick = () => ($("newSongM").hidden = true);
  $("nsName").addEventListener("input", () => { paintNewSong(); clearTimeout(ns.t); ns.t = setTimeout(checkNewName, 150); });
  $("nsName").addEventListener("keydown", (e) => { if (e.key === "Enter") createNewSong(); });
  $("nsBrowse").onclick = async () => { const f = await api.pick_folder(ns.location || prefs.v?.song_location || null); if (f) { ns.location = f; paintNewSong(); } };
  $("nsDefault").onclick = () => { ns.location = ""; paintNewSong(); };
  for (const b of $("nsStart").querySelectorAll("button")) b.onclick = () => { ns.start = b.dataset.v; paintNewSong(); };
  $("nsGo").onclick = createNewSong;
  $("newSongM").addEventListener("keydown", (e) => { if (e.key === "Escape") $("newSongM").hidden = true; e.stopPropagation(); });
  // preferences
  closeOn("prefsM", "pfX");
  $("prefsM").addEventListener("keydown", (e) => { if (e.key === "Escape") $("prefsM").hidden = true; e.stopPropagation(); });
  $("pfBrowse").onclick = async () => { const f = await api.pick_folder(prefs.v?.song_location || null); if (f) { await setPref("song_location", f); openPrefs(); } };
  $("pfLocDefault").onclick = async () => { await setPref("song_location", ""); openPrefs(); };
  $("pfBpm").onchange = () => { const v = parseFloat(String($("pfBpm").value).replace(",", ".")); if (isFinite(v)) setPref("new_bpm", v); };
  $("pfBeats").onchange = () => setPref("new_beats", +$("pfBeats").value);
  $("pfConfirm").onchange = () => setPref("confirm_close", $("pfConfirm").checked);
  $("pfUpdate").onchange = () => setPref("auto_update", $("pfUpdate").checked);
  $("pfThreads").onchange = () => setPref("mix_threads", $("pfThreads").checked);
  $("pfSandbox").onchange = async () => { await setPref("plugin_sandbox", $("pfSandbox").checked); toast("Restart Duet for this to take effect."); };
  $("pfSound").onclick = () => { $("prefsM").hidden = true; openPanel("sound"); };
  $("pfKeys").onclick = () => { $("prefsM").hidden = true; openShortcuts(); };
  // closing
  $("clCancel").onclick = () => ($("closeM").hidden = true);
  $("clClose").onclick = () => api.app_close(false);
  $("clSave").onclick = async () => {
    const r = await api.app_close(!!(song.view && song.view.unsaved));
    if (r && r.error) toast(r.error, true);
  };
  $("closeM").addEventListener("keydown", (e) => { if (e.key === "Escape") $("closeM").hidden = true; e.stopPropagation(); });
}
