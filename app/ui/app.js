const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const byId = (id) => document.getElementById(id);
const accountList = byId("accounts");
const providerList = byId("providers");
const empty = byId("empty");
const notice = byId("notice");
const refreshButton = byId("refresh");
const addButton = byId("add-button");
const dialogs = { add: byId("add"), edit: byId("edit"), provider: byId("provider"), settings: byId("settings") };

const TABS = {
  claude: { name: "Claude Code", unit: "account", add: "Add account", title: "No Claude Code accounts yet", text: "Each account lives in its own isolated home, so one session never signs another out." },
  codex: { name: "Codex", unit: "account", add: "Add account", title: "No Codex accounts yet", text: "Each account lives in its own isolated home, so one session never signs another out." },
  keys: { name: "API keys", unit: "key", add: "Add API key", title: "No API keys yet", text: "Keep one key per provider or router and open it in Claude Code, Codex or any other tool." },
};
const MORE_ICON = "M5 12h.01M12 12h.01M19 12h.01";
const TOOL_NAMES = { claude: "Claude Code", codex: "Codex" };

let tab = "claude";
let rows = [];
let providers = [];
let presets = [];
let editing = null;
let editingProvider = null;

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

let toastTimer = 0;

function say(message, bad = false) {
  clearTimeout(toastTimer);
  notice.textContent = message;
  notice.classList.toggle("bad", bad);
  notice.classList.toggle("show", Boolean(message));
  if (message) toastTimer = setTimeout(() => notice.classList.remove("show"), bad ? 9000 : 3500);
}

function moreButton(label, action) {
  const icon = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  icon.setAttribute("viewBox", "0 0 24 24");
  icon.setAttribute("aria-hidden", "true");
  const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
  path.setAttribute("d", MORE_ICON);
  icon.append(path);
  const button = element("button", { type: "button", class: "more", "aria-label": `Edit ${label}`, title: "Edit" }, [icon]);
  button.addEventListener("click", action);
  return button;
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
  const reset = untilReset(window.resets_at);
  return element(
    "div",
    { class: "stat", "data-level": level, role: "meter", "aria-label": title, "aria-valuemin": "0", "aria-valuemax": "100", "aria-valuenow": String(percent) },
    [
      element("span", { class: "stat-top" }, [element("span", { text: name }), element("b", { text: `${percent}%` })]),
      element("span", { class: "track" }, [fill]),
      element("span", { class: "stat-foot", text: reset ? `resets in ${reset}` : "" }),
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

function accountRow(row) {
  const info = subtitle(row);
  const launch = () => act(() => invoke("open_account", { id: row.id }), `Opened ${row.label}`);
  const open = element("button", { type: "button", class: "open", title: `Open ${row.label} in a terminal` }, [
    element("span", { class: "badge", text: row.harness === "claude" ? "CC" : "CX" }),
    element("span", { class: "who" }, [
      element("strong", { text: row.label }),
      element("span", { text: info.text, class: info.alert ? "alert" : "" }),
    ]),
    element("span", { class: "meters" }, meters(row)),
  ]);
  const go = element("button", { type: "button", class: "quiet go", text: "Open", tabindex: "-1" });
  open.addEventListener("click", launch);
  go.addEventListener("click", launch);
  return element("li", { class: "row" }, [open, go, moreButton(row.label, () => edit(row))]);
}

function toolsOf(provider) {
  const tools = [];
  if (provider.anthropic_url) tools.push("claude");
  if (provider.openai_url) tools.push("codex");
  tools.push("shell");
  return tools;
}

function providerRow(provider) {
  const hosts = [...new Set([provider.anthropic_url, provider.openai_url].filter(Boolean).map(host))];
  const head = element("div", { class: "head" }, [
    element("span", { class: "badge", text: provider.label.slice(0, 2).toUpperCase() }),
    element("span", { class: "who" }, [
      element("strong", { text: provider.label }),
      element("span", { text: [...hosts, provider.key_env].join(" · ") }),
    ]),
  ]);
  const launch = element("div", { class: "launch" });
  let select = null;
  if (provider.models.length) {
    select = element("select", { "aria-label": `Model for ${provider.label}` }, provider.models.map((model) => element("option", { text: model, value: model })));
    launch.append(select);
  }
  launch.append(element("span", { class: "with", text: "open in" }));
  for (const tool of toolsOf(provider)) {
    const name = TOOL_NAMES[tool] ?? provider.command[0] ?? "Shell";
    const button = element("button", { type: "button", class: "tool", text: name, title: `Open ${provider.label} in ${name}` });
    button.addEventListener("click", () =>
      act(() => invoke("open_provider", { id: provider.id, tool, model: select?.value ?? null }), `Opened ${provider.label} in ${name}`),
    );
    launch.append(button);
  }
  return element("li", { class: "row provider" }, [head, moreButton(provider.label, () => editProvider(provider)), launch]);
}

function render() {
  const visible = rows.filter((row) => row.harness === tab);
  const counts = { claude: 0, codex: 0, keys: providers.length };
  for (const row of rows) counts[row.harness] += 1;
  for (const button of document.querySelectorAll("[data-tab]")) {
    button.setAttribute("aria-selected", String(button.dataset.tab === tab));
    button.querySelector(".count").textContent = counts[button.dataset.tab] || "";
  }
  const onKeys = tab === "keys";
  const total = counts[tab];
  const section = TABS[tab];
  byId("page-title").textContent = section.name;
  byId("page-caption").textContent = total ? `${total} ${section.unit}${total === 1 ? "" : "s"}` : "nothing here yet";
  accountList.hidden = onKeys || !total;
  providerList.hidden = !onKeys || !total;
  accountList.replaceChildren(...visible.map(accountRow));
  providerList.replaceChildren(...providers.map(providerRow));
  empty.hidden = total > 0;
  addButton.hidden = total === 0;
  empty.querySelector("h2").textContent = section.title;
  empty.querySelector("p").textContent = section.text;
  addButton.textContent = section.add;
  byId("empty-add").textContent = section.add;
}

async function load(refresh = false) {
  refreshButton.classList.toggle("spinning", refresh);
  try {
    [rows, providers] = await Promise.all([invoke("accounts", { refresh }), invoke("providers")]);
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
  } catch (error) {
    say(String(error), true);
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
  const form = byId("add-form");
  const mode = form.elements.mode.value;
  for (const node of form.querySelectorAll("[data-mode]")) {
    node.hidden = !node.dataset.mode.split(" ").includes(mode);
  }
  form.elements.key.required = mode === "key";
}

function openAdd() {
  if (tab === "keys") return editProvider(null);
  const form = byId("add-form");
  form.reset();
  showMode();
  byId("add-title").textContent = `Add a ${TOOL_NAMES[tab]} account`;
  form.querySelector(".error").textContent = "";
  dialogs.add.showModal();
  form.elements.label.focus();
}

function edit(row) {
  editing = row;
  const form = byId("edit-form");
  const kinds = { key: row.auth.kind === "api_key", login: row.auth.kind === "login", managed: !row.native_home };
  for (const node of form.querySelectorAll("[data-only]")) node.hidden = !kinds[node.dataset.only];
  form.elements.label.value = row.label;
  form.elements.model.value = row.model ?? "";
  form.elements.fast_model.value = row.fast_model ?? "";
  form.elements.key.value = "";
  form.elements.kit.checked = row.kit;
  form.querySelector(".error").textContent = "";
  dialogs.edit.showModal();
  form.elements.label.focus();
}

function fillProvider(values) {
  const form = byId("provider-form");
  for (const name of ["label", "key_env", "anthropic_url", "openai_url"]) {
    if (values[name] !== undefined) form.elements[name].value = values[name] ?? "";
  }
}

function editProvider(provider) {
  editingProvider = provider;
  const form = byId("provider-form");
  form.reset();
  byId("provider-title").textContent = provider ? provider.label : "Add API key";
  byId("presets").hidden = Boolean(provider);
  byId("advanced").open = Boolean(provider);
  for (const node of form.querySelectorAll("[data-only=edit]")) node.hidden = !provider;
  form.elements.key.required = !provider;
  if (provider) {
    fillProvider(provider);
    form.elements.models.value = provider.models.join(" ");
    form.elements.command.value = provider.command.join(" ");
  } else if (presets.length) {
    form.elements.preset.value = presets[0].name;
    fillProvider(presets[0]);
  }
  form.querySelector(".error").textContent = "";
  dialogs.provider.showModal();
  form.elements[provider ? "label" : "key"].focus();
}

function disarm(button) {
  delete button.dataset.armed;
  button.textContent = "Remove";
}

function confirmed(button) {
  if (button.dataset.armed) return true;
  button.dataset.armed = "yes";
  button.textContent = "Click again to remove";
  return false;
}

byId("add-form").addEventListener("change", showMode);

byId("add-form").addEventListener("submit", (event) => {
  event.preventDefault();
  const form = event.currentTarget;
  const value = (name) => form.elements[name].value.trim();
  const mode = form.elements.mode.value;
  const task =
    mode === "adopt"
      ? () => invoke("adopt_account", { label: value("label"), harness: tab, dir: value("dir") || null })
      : () =>
          invoke("add_account", {
            form: {
              label: value("label"),
              harness: tab,
              api_key: mode === "key",
              base_url: value("base_url") || null,
              bearer: form.elements.bearer.checked,
              model: value("model") || null,
              fast_model: value("fast_model") || null,
              key: mode === "key" ? form.elements.key.value : null,
            },
          });
  const done = mode === "login" ? "Finish signing in on the terminal that opened" : `Added ${value("label")}`;
  submit(dialogs.add, task, done);
});

byId("edit-form").addEventListener("submit", (event) => {
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

byId("remove").addEventListener("click", (event) => {
  if (!confirmed(event.currentTarget)) return;
  submit(dialogs.edit, () => invoke("remove_account", { id: editing.id, purge: false }), `Removed ${editing.label}; its folder was kept`);
});

byId("relogin").addEventListener("click", () => {
  dialogs.edit.close();
  act(() => invoke("login_account", { id: editing.id }), "Finish signing in on the terminal that opened");
});

byId("provider-form").addEventListener("change", (event) => {
  if (event.target.name !== "preset") return;
  const preset = presets.find((entry) => entry.name === event.target.value);
  byId("advanced").open = !preset;
  fillProvider(preset ?? { label: "", key_env: "", anthropic_url: "", openai_url: "" });
});

byId("provider-form").addEventListener("submit", (event) => {
  event.preventDefault();
  const form = event.currentTarget;
  const value = (name) => form.elements[name].value.trim();
  const words = (name) => value(name).split(/[\s,]+/).filter(Boolean);
  const payload = {
    label: value("label"),
    key_env: value("key_env"),
    anthropic_url: value("anthropic_url") || null,
    openai_url: value("openai_url") || null,
    models: words("models"),
    command: words("command"),
    key: form.elements.key.value || null,
  };
  const current = editingProvider;
  const task = current ? () => invoke("update_provider", { id: current.id, form: payload }) : () => invoke("add_provider", { form: payload });
  submit(dialogs.provider, task, current ? "Saved" : `Added ${payload.label}`).then(() => {
    form.elements.key.value = "";
  });
});

byId("remove-provider").addEventListener("click", (event) => {
  if (!confirmed(event.currentTarget)) return;
  submit(dialogs.provider, () => invoke("remove_provider", { id: editingProvider.id }), `Removed ${editingProvider.label}`);
});

dialogs.edit.addEventListener("close", () => disarm(byId("remove")));
dialogs.provider.addEventListener("close", () => disarm(byId("remove-provider")));

byId("open-settings").addEventListener("click", async () => {
  const form = byId("settings-form");
  try {
    const current = await invoke("settings");
    form.elements.claude_kit.value = current.claude_kit ?? "";
    form.elements.codex_kit.value = current.codex_kit ?? "";
    form.elements.terminal.value = current.terminal.join(" ");
    form.querySelector(".error").textContent = "";
    dialogs.settings.showModal();
    form.elements.claude_kit.focus();
  } catch (error) {
    say(String(error), true);
  }
});

byId("settings-form").addEventListener("submit", (event) => {
  event.preventDefault();
  const form = event.currentTarget;
  const settings = {
    claude_kit: form.elements.claude_kit.value.trim() || null,
    codex_kit: form.elements.codex_kit.value.trim() || null,
    terminal: form.elements.terminal.value.trim().split(/\s+/).filter(Boolean),
  };
  submit(dialogs.settings, () => invoke("save_settings", { settings }), "Settings saved");
});

for (const button of document.querySelectorAll("[data-tab]")) {
  button.addEventListener("click", () => {
    tab = button.dataset.tab;
    say("");
    render();
  });
}

for (const button of document.querySelectorAll("[data-close]")) {
  button.addEventListener("click", () => button.closest("dialog").close());
}

async function start() {
  try {
    presets = await invoke("provider_presets");
  } catch {
    presets = [];
  }
  const choices = [...presets.map((preset) => [preset.name, preset.label]), ["custom", "Custom"]];
  byId("presets").replaceChildren(
    ...choices.map(([value, label]) =>
      element("label", {}, [element("input", { type: "radio", name: "preset", value }), element("span", {}, [element("strong", { text: label })])]),
    ),
  );
  showMode();
  await load();
}

addButton.addEventListener("click", openAdd);
byId("empty-add").addEventListener("click", openAdd);
refreshButton.addEventListener("click", () => load(true));
document.addEventListener("contextmenu", (event) => event.preventDefault());
listen("ledger-changed", () => load());
start();
