// Composed by the idealyst web build from the service-worker handlers the
// app's dependencies declare under [package.metadata.idealyst.web]. Do not
// edit: it is rewritten on every build.
//
// This prelude is the worker half of `web_glue::service_worker`:
//
//   self.idealyst.deliver(channel, message)  -> Promise
//       Hand `message` to the page's `on_message(channel, …)` handler. It is
//       written to an IndexedDB outbox first, then sent to one page that has
//       said it is listening (the focused one, else a visible one, else any).
//       With no such page — none open, or one still booting — it stays in
//       the outbox, which survives the worker being stopped, and goes out
//       when a page sends "ready".
//   self.idealyst.on(channel, (message, client) => …)
//       Receive what the page `post`s on `channel`.
//
// Wire messages are `{ idealyst: "ready" | "ping" | "message", channel?,
// message? }`. A page sends "ready" once its listener is installed (a
// message posted before that is lost); the worker sends "ping" when it has
// mail but knows no ready page — its memory of ready pages dies with it —
// and a listening page answers with "ready".
"use strict";
self.idealyst = self.idealyst || {};
(() => {
  const DB = "idealyst-sw";
  const OUTBOX = "outbox";
  const readyClients = new Set();
  const handlers = new Map();

  function open() {
    return new Promise((resolve, reject) => {
      const req = self.indexedDB.open(DB, 1);
      req.onupgradeneeded = () => req.result.createObjectStore(OUTBOX, { autoIncrement: true });
      req.onsuccess = () => resolve(req.result);
      req.onerror = () => reject(req.error);
    });
  }

  // Run `body(store)` in one transaction; resolves with what `body`
  // returned (a request's result once the transaction commits).
  async function inOutbox(mode, body) {
    const db = await open();
    try {
      return await new Promise((resolve, reject) => {
        const tx = db.transaction(OUTBOX, mode);
        const out = body(tx.objectStore(OUTBOX));
        tx.oncomplete = () => resolve(out && "result" in out ? out.result : out);
        tx.onerror = () => reject(tx.error);
        tx.onabort = () => reject(tx.error);
      });
    } finally {
      db.close();
    }
  }

  // Everything queued, removed in the same transaction — two flushes
  // racing can never send one message twice.
  function takeAll() {
    return inOutbox("readwrite", (store) => {
      const all = store.getAll();
      all.onsuccess = () => store.clear();
      return all;
    });
  }

  async function target() {
    const clients = await self.clients.matchAll({ type: "window", includeUncontrolled: true });
    const ready = clients.filter((c) => readyClients.has(c.id));
    const rank = (c) => (c.focused ? 0 : c.visibilityState === "visible" ? 1 : 2);
    ready.sort((a, b) => rank(a) - rank(b));
    return { client: ready[0], clients };
  }

  async function flush() {
    const { client, clients } = await target();
    if (!client) {
      for (const c of clients) c.postMessage({ idealyst: "ping" });
      return;
    }
    for (const entry of await takeAll()) {
      client.postMessage({ idealyst: "message", channel: entry.channel, message: entry.message });
    }
  }

  self.idealyst.deliver = async (channel, message) => {
    await inOutbox("readwrite", (store) => store.add({ channel, message }));
    await flush();
  };

  self.idealyst.on = (channel, handler) => {
    handlers.set(channel, handler);
  };

  self.addEventListener("message", (event) => {
    const data = event.data;
    if (!data || typeof data.idealyst !== "string") return;
    if (data.idealyst === "ready" && event.source) {
      readyClients.add(event.source.id);
      event.waitUntil(flush());
    } else if (data.idealyst === "message") {
      const handler = handlers.get(data.channel);
      if (handler) event.waitUntil(Promise.resolve(handler(data.message, event.source)));
    }
  });

  // A new version takes over at once: contributions are event handlers,
  // with no cache an open page depends on, so a fixed handler should not
  // wait for every tab to close.
  self.addEventListener("install", () => self.skipWaiting());
  self.addEventListener("activate", (event) => event.waitUntil(self.clients.claim()));
})();
