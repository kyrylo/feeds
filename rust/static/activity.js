(() => {
  "use strict";
  const csrf = document.querySelector('meta[name="csrf-token"]')?.content;
  const zone = Intl.DateTimeFormat().resolvedOptions().timeZone;
  if (zone) document.cookie = `telesink_time_zone=${encodeURIComponent(zone)}; Path=/; SameSite=Lax; Max-Age=31536000`;
  const compactLog = window.matchMedia("(max-width: 700px)");
  const relative = new Intl.RelativeTimeFormat(undefined, { numeric: "auto" });
  function times(root = document) {
    for (const time of root.querySelectorAll("time[datetime]")) {
      const date = new Date(time.dateTime);
      if (!Number.isFinite(date.getTime())) continue;
      time.title = date.toLocaleString();
      if (time.hasAttribute("data-log-time")) {
        const clock = date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit", hour12: false });
        time.textContent = compactLog.matches ? clock : `${date.toLocaleDateString("en", { month: "short", day: "2-digit" })} ${clock}`;
        continue;
      }
      if (time.hasAttribute("data-absolute")) { time.textContent = date.toLocaleString(); continue; }
      const seconds = (date.getTime() - Date.now()) / 1000;
      const [unit, size] = Math.abs(seconds) < 60 ? ["second", 1] : Math.abs(seconds) < 3600 ? ["minute", 60] : Math.abs(seconds) < 86400 ? ["hour", 3600] : ["day", 86400];
      time.textContent = Math.abs(seconds) > 7 * 86400 ? date.toLocaleDateString(undefined, { month: "short", day: "numeric", year: date.getFullYear() === new Date().getFullYear() ? undefined : "numeric" }) : relative.format(Math.round(seconds / size), unit);
    }
  }
  times();
  compactLog.addEventListener("change", () => times());
  setInterval(() => times(), 30000);
  document.querySelectorAll("[data-auto-submit]").forEach(select => select.addEventListener("change", () => select.form.requestSubmit()));
  document.querySelectorAll("form[data-confirm]").forEach(form => form.addEventListener("submit", event => { if (!window.confirm(form.dataset.confirm)) event.preventDefault(); }));

  document.addEventListener("click", async event => {
    const button = event.target.closest("[data-copy-target],[data-copy-input]");
    if (!button) return;
    const target = document.getElementById(button.dataset.copyInput || button.dataset.copyTarget);
    const text = button.dataset.copyInput ? target?.value : target?.textContent;
    if (!text) return;
    const original = button.textContent;
    try {
      await navigator.clipboard.writeText(text);
      button.textContent = "Copied";
    } catch {
      button.textContent = "Select and copy";
      if (target instanceof HTMLInputElement) { target.type = "text"; target.select(); }
      else if (target) { const selection = window.getSelection(); const range = document.createRange(); range.selectNodeContents(target); selection.removeAllRanges(); selection.addRange(range); }
    }
    setTimeout(() => { button.textContent = original; }, 2000);
  });
  document.addEventListener("toggle", async event => {
    const details = event.target;
    if (!details.matches?.(".activity-event") || !details.open) return;
    const container = details.querySelector("[data-details-url]");
    if (!container || container.dataset.loaded || container.dataset.loading) return;
    container.dataset.loading = "true";
    try {
      const response = await fetch(container.dataset.detailsUrl, { headers: { Accept: "text/html" }, redirect: "error" });
      if (!response.ok) throw new Error("Details unavailable");
      container.innerHTML = await response.text();
      container.dataset.loaded = "true";
      times(container);
    } catch {
      if (!container.querySelector(".detail-error")) {
        const message = document.createElement("p"); message.className = "muted detail-error"; message.textContent = "Couldn’t load details. Close and reopen to retry, or use the event link."; container.prepend(message);
      }
    } finally { delete container.dataset.loading; }
  }, true);

  const shortcuts = document.getElementById("shortcuts");
  document.querySelector("[data-show-shortcuts]")?.addEventListener("click", () => shortcuts.showModal());
  document.querySelector("[data-close-shortcuts]")?.addEventListener("click", () => shortcuts.close());
  shortcuts?.addEventListener("click", event => { if (event.target === shortcuts && event.offsetX < 0) shortcuts.close(); });
  const channelsButton = document.querySelector("[data-toggle-feeds]");
  function closeChannels() {
    document.body.classList.remove("feeds-open");
    channelsButton?.setAttribute("aria-expanded", "false");
  }
  channelsButton?.addEventListener("click", () => {
    const open = document.body.classList.toggle("feeds-open");
    channelsButton.setAttribute("aria-expanded", String(open));
  });
  document.addEventListener("keydown", event => {
    if (event.defaultPrevented || event.metaKey || event.ctrlKey || event.altKey) return;
    if (shortcuts?.open) return;
    if (event.key === "Escape") {
      closeChannels();
      if (event.target.matches("input,select,textarea")) { event.target.blur(); return; }
      const details = document.activeElement?.closest(".activity-event[open]");
      if (details) { details.open = false; details.querySelector("summary").focus(); }
      return;
    }
    if (event.target.closest("input,select,textarea,[contenteditable=true]")) return;
    if (event.key === "?") { event.preventDefault(); shortcuts?.showModal(); }
    if (event.key === "/" && document.getElementById("q")) { event.preventDefault(); document.getElementById("q").focus(); }
    if ((event.key === "j" || event.key === "k") && document.getElementById("activity-list")) {
      const rows = [...document.querySelectorAll("#activity-list .event-row")];
      if (!rows.length) return;
      event.preventDefault();
      const current = rows.indexOf(document.activeElement);
      const index = current < 0 ? (event.key === "j" ? 0 : rows.length - 1) : Math.max(0, Math.min(rows.length - 1, current + (event.key === "j" ? 1 : -1)));
      rows[index].focus({ preventScroll: true }); rows[index].scrollIntoView({ block: "nearest" });
    }
  });

  const activity = document.getElementById("activity");
  if (!activity) return;
  document.querySelector('.activity-tab[aria-current="page"]')?.scrollIntoView({block:"nearest", inline:"nearest"});
  const list = document.getElementById("activity-list");
  const older = document.getElementById("load-older");
  let cursor = activity.dataset.cursor;
  let seenToken = activity.dataset.seenToken;
  let markedToken = "";
  let markingSeen;
  let newCount = Number(document.getElementById("new-count").textContent);
  let fetching = false;
  let stopped = false;
  const log = document.getElementById("log-scroll");
  let following = true;
  function atBottom() { return log.scrollHeight - log.clientHeight - log.scrollTop <= 24; }
  log.addEventListener("scroll", () => { following = atBottom(); }, { passive: true });
  new ResizeObserver(() => { if (following) log.scrollTop = log.scrollHeight; }).observe(log);
  let timer;
  function unreadCounts(counts) {
    if (!counts) return;
    const values = new Map([["all", counts.all], ...counts.tabs.map(tab => [tab.id, tab.unread])]);
    for (const link of document.querySelectorAll(".activity-tab[data-tab-id]")) {
      const count = values.get(link.dataset.tabId);
      if (!Number.isSafeInteger(count) || count < 0) continue;
      const badge = link.querySelector(".activity-tab__count");
      badge.dataset.unread = String(count);
      badge.textContent = count > 999 ? "999+" : count.toLocaleString();
      badge.hidden = count === 0;
      const label = `${count.toLocaleString()} unread ${count === 1 ? "event" : "events"}`;
      badge.setAttribute("aria-label", label); badge.title = label;
    }
  }
  function catchUp() {
    document.getElementById("new-count").textContent = newCount.toLocaleString();
    document.getElementById("new-noun").textContent = newCount === 1 ? "event" : "events";
    document.getElementById("catch-up").hidden = newCount === 0;
    list.querySelectorAll(".seen-divider").forEach(node => node.remove());
    const rows = [...list.querySelectorAll(".activity-event")];
    const firstNew = rows.findIndex(row => row.dataset.new === "true");
    // Late-arriving events can have old timestamps. Do not draw a misleading
    // time divider when new and previously seen events are interleaved.
    if (firstNew > 0 && rows.slice(firstNew).every(row => row.dataset.new === "true")) {
      const divider = document.createElement("div"); divider.className = "seen-divider"; divider.textContent = "New since your last visit"; rows[firstNew].before(divider);
    }
  }
  function scrollPosition() {
    const anchor = [...list.querySelectorAll(".activity-event")].find(node => node.getBoundingClientRect().bottom > log.getBoundingClientRect().top);
    return { anchor, top: anchor?.getBoundingClientRect().top, atBottom: atBottom() };
  }
  function insert(html, live, position = scrollPosition()) {
    const fragment = document.createElement("template"); fragment.innerHTML = html;
    let added = 0;
    for (const node of [...fragment.content.children]) {
      if (!node.matches(".activity-event") || document.getElementById(node.id)) continue;
      list.append(node); added++;
      if (live && node.dataset.new === "true") newCount++;
    }
    if (added) {
      const rows = [...list.querySelectorAll(".activity-event")];
      rows.sort((a, b) => {
        // RFC3339 strings use consistent UTC formatting; Date normalizes subsecond
        // precision and IDs break timestamp ties without lossy Number conversion.
        const time = new Date(a.dataset.time) - new Date(b.dataset.time);
        if (time) return time;
        const at = a.dataset.time.replace(/Z$/, "").split(".")[1] || "";
        const bt = b.dataset.time.replace(/Z$/, "").split(".")[1] || "";
        const fraction = at.padEnd(9, "0").localeCompare(bt.padEnd(9, "0"));
        if (fraction) return fraction;
        return BigInt(a.dataset.eventId) > BigInt(b.dataset.eventId) ? 1 : -1;
      });
      rows.forEach(node => list.append(node));
      document.getElementById("activity-empty").hidden = true;
      times(list); catchUp();
    }
    if (live && position.atBottom) log.scrollTop = log.scrollHeight;
    else if (position.anchor && position.top !== undefined) log.scrollTop += position.anchor.getBoundingClientRect().top - position.top;
  }
  async function markSeen() {
    if (markingSeen) await markingSeen;
    if (!seenToken || seenToken === markedToken || document.hidden) return;
    const token = seenToken;
    markingSeen = (async () => {
      try {
        const response = await fetch("/activity/seen", { method: "POST", headers: { "Content-Type": "application/x-www-form-urlencoded", "X-CSRF-Token": csrf }, body: new URLSearchParams({ seen_token: token }), redirect: "error" });
        if (response.ok) {
          if (response.status !== 204) unreadCounts((await response.json()).counts);
          markedToken = token;
        }
      } catch { /* Retry with the next successful update. */ }
    })();
    await markingSeen; markingSeen = null;
  }
  async function poll() {
    if (fetching || stopped || document.hidden) return;
    fetching = true;
    let next = 3000;
    try {
      const path = new URL(activity.dataset.updatesUrl, location.origin);
      path.searchParams.set("after", cursor); path.searchParams.set("seen", activity.dataset.seen);
      const response = await fetch(path, { headers: { Accept: "application/json" }, redirect: "manual" });
      if (response.type === "opaqueredirect" || response.status === 401 || response.status === 403) {
        stopped = true; window.location.assign("/session/new"); return;
      }
      if (!response.ok) throw new Error("Connection unavailable");
      const data = await response.json();
      if (document.hidden) return;
      insert(data.html, true); cursor = data.cursor; seenToken = data.seen_token;
      unreadCounts(data.counts);
      const channels = document.querySelector(".feed-links");
      const existingSources = new Set([...channels.querySelectorAll("[data-source-id]")].map(link => link.dataset.sourceId));
      for (const item of data.sources) if (!existingSources.has(item.id)) {
        const link = document.createElement("a"); link.href = `/?source=${encodeURIComponent(item.id)}`; link.dataset.sourceId = item.id; link.className = "activity-tab";
        const prefix = document.createElement("span"); prefix.className = "feed-prefix"; prefix.textContent = "#"; prefix.setAttribute("aria-hidden", "true");
        const name = document.createElement("span"); name.className = "activity-tab__name"; name.textContent = item.name;
        link.append(prefix, name); channels.append(link);
      }
      if (data.sources.length) document.querySelector(".sidebar-empty")?.remove();
      await markSeen();
      if (data.more) next = 200;
    } catch { next = 5000; }
    finally { fetching = false; if (!stopped && !document.hidden) timer = setTimeout(poll, next); }
  }
  older.addEventListener("click", async event => {
    event.preventDefault();
    if (older.getAttribute("aria-busy") === "true") return;
    older.setAttribute("aria-busy", "true"); older.textContent = "Loading…";
    const message = document.getElementById("page-message"); message.textContent = "";
    try {
      const response = await fetch(older.href, { headers: { Accept: "application/json" }, redirect: "error" });
      if (!response.ok) throw new Error("Page unavailable");
      const data = await response.json();
      const position = scrollPosition();
      older.hidden = !data.older_url;
      if (data.older_url) older.href = data.older_url;
      insert(data.html, false, position);
    } catch { message.textContent = "Couldn’t load earlier activity. Try again."; }
    finally { older.removeAttribute("aria-busy"); older.textContent = "Load earlier events"; }
  });
  document.addEventListener("visibilitychange", async () => {
    clearTimeout(timer);
    if (!document.hidden) { await markSeen(); poll(); }
  });
  window.addEventListener("pageshow", event => { if (event.persisted) { stopped = false; clearTimeout(timer); poll(); } });
  catchUp(); log.scrollTop = log.scrollHeight; markSeen(); timer = setTimeout(poll, 3000);
})();
