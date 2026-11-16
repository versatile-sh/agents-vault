const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const list = document.getElementById("accounts");
const empty = document.getElementById("empty");
const notice = document.getElementById("notice");
const refreshButton = document.getElementById("refresh");
const dialogs = {
  add: document.getElementById("add"),
  edit: document.getElementById("edit"),
  settings: document.getElementById("settings"),
};

let rows = [];
let editing = null;

function element(tag, properties = {}, children = []) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(properties)) {
    if (key === "text") node.textContent = value;
    else if (key === "class") node.className = value;
    else node.setAttribute(key, value);
  }
  node.append(...children);
  return node;
}

function say(message, bad = false) {
  notice.textContent = message;
  notice.classList.toggle("bad", bad);
}

function untilReset(resetsAt) {
  if (!resetsAt) return "";
  const minutes = Math.max(0, Math.floor((resetsAt - Date.now() / 1000) / 60));
  if (minutes >= 2880) return `${Math.floor(minutes / 1440)}d`;
  if (minutes >= 60) return `${Math.floor(minutes / 60)}h${String(minutes % 60).padStart(2, "0")}`;
  return `${minutes}m`;
}

function meter(name, title, window) {
  const percent = Math.round(window.percent);
  const level = percent >= 90 ? "high" : percent >= 70 ? "warn" : "ok";
  const fill = element("span", { class: "fill" });
  fill.style.width = `${Math.max(percent, 2)}%`;
  return element(
    "div",
    { class: "meter", "data-level": level, role: "meter", "aria-label": title, "aria-valuemin": "0", "aria-valuemax": "100", "aria-valuenow": String(percent) },
    [
      element("span", { text: name }),
      element("span", { class: "track" }, [fill]),
      element("span", { class: "value", text: `${percent}%` }),
      element("span", { class: "reset", text: untilReset(window.resets_at) }),
    ],
  );
}

function host(url) {
  try {
    return new URL(url).host;
  } catch {
    return url;
  }
}

function subtitle(row) {
  if (row.auth.kind === "api_key") {
    const where = row.auth.base_url ? host(row.auth.base_url) : "API key";
    return { text: [where, row.model].filter(Boolean).join(" · ") };
  }
  if (row.status && !row.status.logged_in) return { text: "Needs sign-in", alert: true };
  const parts = [row.status?.email, row.status?.plan, row.model].filter(Boolean);
  return { text: parts.join(" · ") || "Signed in through the harness" };
}

function meters(row) {
  const limits = row.limits ?? {};
  const items = [];
  if (limits.session) items.push(meter("5h", "5-hour limit used", limits.session));
  if (limits.weekly) items.push(meter("7d", "Weekly limit used", limits.weekly));
  if (items.length) return items;
  const text = row.auth.kind === "api_key" ? "Pay as you go" : "Limits appear after the first session";
  return [element("span", { class: "plain", text })];
}

function render() {
  empty.hidden = rows.length > 0;
  list.replaceChildren(
    ...rows.map((row) => {
      const info = subtitle(row);
      const open = element("button", { type: "button", class: "open", title: `Open ${row.label} in a terminal` }, [
        element("span", { class: "badge", "data-harness": row.harness, text: row.harness === "claude" ? "CC" : "CX" }),
        element("span", { class: "who" }, [
          element("strong", { text: row.label }),
          element("span", { text: info.text, class: info.alert ? "alert" : "" }),
        ]),
        element("span", { class: "meters" }, meters(row)),
      ]);
      open.addEventListener("click", () => act(() => invoke("open_account", { id: row.id }), `Opened ${row.label}`));
      const more = element("button", { type: "button", class: "more", text: "⋯", "aria-label": `Edit ${row.label}` });
      more.addEventListener("click", () => edit(row));
      return element("li", { class: "account" }, [open, more]);
    }),
  );
}

async function load(refresh = false) {
  refreshButton.classList.toggle("spinning", refresh);
  try {
    rows = await invoke("accounts", { refresh });
    render();
  } catch (error) {
    say(String(error), true);
  } finally {
    refreshButton.classList.remove("spinning");
  }
}

async function act(task, done) {
  try {
    await task();
    say(done);
    return true;
  } catch (error) {
    say(String(error), true);
    return false;
  }
}

async function submit(dialog, task, done) {
  const error = dialog.querySelector(".error");
  const button = dialog.querySelector("button[type=submit]");
  error.textContent = "";
  button.disabled = true;
  try {
    await task();
    dialog.close();
    say(done);
    await load();
  } catch (failure) {
    error.textContent = String(failure);
  } finally {
    button.disabled = false;
  }
}

function showMode() {
  const form = document.getElementById("add-form");
  const mode = form.elements.mode.value;
  for (const node of form.querySelectorAll("[data-mode]")) {
    node.hidden = !node.dataset.mode.split(" ").includes(mode);
  }
  form.elements.key.required = mode === "key";
}

function edit(row) {
  editing = row;
  const form = document.getElementById("edit-form");
  const kinds = { key: row.auth.kind === "api_key", login: row.auth.kind === "login", managed: !row.native_home };
  for (const node of form.querySelectorAll("[data-only]")) node.hidden = !kinds[node.dataset.only];
  form.elements.label.value = row.label;
  form.elements.model.value = row.model ?? "";
  form.elements.fast_model.value = row.fast_model ?? "";
  form.elements.key.value = "";
  form.elements.kit.checked = row.kit;
  form.querySelector(".error").textContent = "";
  dialogs.edit.showModal();
}

document.getElementById("add-form").addEventListener("change", showMode);

document.getElementById("add-form").addEventListener("submit", (event) => {
  event.preventDefault();
  const form = event.currentTarget;
  const value = (name) => form.elements[name].value.trim();
  const mode = form.elements.mode.value;
  const harness = form.elements.harness.value;
  const task =
    mode === "adopt"
      ? () => invoke("adopt_account", { label: value("label"), harness, dir: value("dir") || null })
      : () =>
          invoke("add_account", {
            form: {
              label: value("label"),
              harness,
              api_key: mode === "key",
              base_url: value("base_url") || null,
              bearer: form.elements.bearer.checked,
              model: value("model") || null,
              fast_model: value("fast_model") || null,
              key: mode === "key" ? form.elements.key.value : null,
            },
          });
  const done = mode === "login" ? "Finish signing in on the terminal that opened" : `Added ${value("label")}`;
  submit(dialogs.add, task, done).then(() => {
    if (!dialogs.add.open) form.reset();
    showMode();
  });
});

document.getElementById("edit-form").addEventListener("submit", (event) => {
  event.preventDefault();
  const form = event.currentTarget;
  const row = editing;
  submit(
    dialogs.edit,
    async () => {
      const patch = {
        label: form.elements.label.value.trim(),
        model: form.elements.model.value.trim() || null,
        fast_model: form.elements.fast_model.value.trim() || null,
        kit: form.elements.kit.checked,
      };
      await invoke("update_account", { id: row.id, patch });
      if (form.elements.key.value) await invoke("set_key", { id: row.id, key: form.elements.key.value });
      form.elements.key.value = "";
    },
    "Saved",
  );
});

document.getElementById("remove").addEventListener("click", (event) => {
  const button = event.currentTarget;
  if (button.dataset.armed !== editing.id) {
    button.dataset.armed = editing.id;
    button.textContent = "Click again to remove";
    return;
  }
  submit(dialogs.edit, () => invoke("remove_account", { id: editing.id, purge: false }), `Removed ${editing.label}; its folder was kept`);
});

dialogs.edit.addEventListener("close", () => {
  const button = document.getElementById("remove");
  delete button.dataset.armed;
  button.textContent = "Remove";
});

document.getElementById("relogin").addEventListener("click", () => {
  dialogs.edit.close();
  act(() => invoke("login_account", { id: editing.id }), "Finish signing in on the terminal that opened");
});

document.getElementById("open-settings").addEventListener("click", async () => {
  const form = document.getElementById("settings-form");
  try {
    const current = await invoke("settings");
    form.elements.claude_kit.value = current.claude_kit ?? "";
    form.elements.codex_kit.value = current.codex_kit ?? "";
    form.elements.terminal.value = current.terminal.join(" ");
    form.querySelector(".error").textContent = "";
    dialogs.settings.showModal();
  } catch (error) {
    say(String(error), true);
  }
});

document.getElementById("settings-form").addEventListener("submit", (event) => {
  event.preventDefault();
  const form = event.currentTarget;
  const settings = {
    claude_kit: form.elements.claude_kit.value.trim() || null,
    codex_kit: form.elements.codex_kit.value.trim() || null,
    terminal: form.elements.terminal.value.trim().split(/\s+/).filter(Boolean),
  };
  submit(dialogs.settings, () => invoke("save_settings", { settings }), "Settings saved");
});

for (const button of document.querySelectorAll("[data-open]")) {
  button.addEventListener("click", () => {
    dialogs[button.dataset.open].querySelector(".error").textContent = "";
    dialogs[button.dataset.open].showModal();
  });
}

for (const button of document.querySelectorAll("[data-close]")) {
  button.addEventListener("click", () => button.closest("dialog").close());
}

refreshButton.addEventListener("click", () => load(true));
document.addEventListener("contextmenu", (event) => event.preventDefault());
listen("ledger-changed", () => load());
showMode();
load();
