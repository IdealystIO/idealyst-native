// The `notifications` SDK's service-worker handlers: what the user does with
// a notification the page showed through `registration.showNotification`
// (src/web.rs). Each response is delivered on the "notifications" channel
// (`self.idealyst.deliver`), which holds it until a page is listening — so a
// click with no tab open still reaches the page it opens.
//
// A notification is ours when its `data.idealyst` is set; `data` carries the
// id, the app payload, and the ids of actions that must not open the app.
const CHANNEL = "notifications";

function ours(notification) {
  const d = notification.data;
  return d && d.idealyst ? d : null;
}

// Focus the most relevant open page, or open the app. Called first thing in
// the click handler: browsers only allow `focus` / `openWindow` while the
// click's user activation lasts.
async function bringUpTheApp() {
  const clients = await self.clients.matchAll({ type: "window", includeUncontrolled: true });
  const rank = (c) => (c.focused ? 0 : c.visibilityState === "visible" ? 1 : 2);
  clients.sort((a, b) => rank(a) - rank(b));
  if (clients[0]) {
    try {
      await clients[0].focus();
      return;
    } catch (_) {
      // Not focusable (no user activation left): the message still goes out.
    }
  }
  if (!clients.length) await self.clients.openWindow(self.registration.scope);
}

self.addEventListener("notificationclick", (event) => {
  const d = ours(event.notification);
  if (!d) return;
  event.notification.close();
  const action = event.action ? "action:" + event.action : "open";
  const background = !!event.action && (d.background || []).includes(event.action);
  const opening = background ? Promise.resolve() : bringUpTheApp();
  const delivered = self.idealyst.deliver(CHANNEL, { id: d.id, action, payload: d.payload || {} });
  event.waitUntil(Promise.all([opening, delivered]));
});

// Fired when the user dismisses the notification — not when the page or this
// worker calls `close()`, and not when it merely leaves the screen.
self.addEventListener("notificationclose", (event) => {
  const d = ours(event.notification);
  if (!d) return;
  event.waitUntil(self.idealyst.deliver(CHANNEL, { id: d.id, action: "dismiss", payload: d.payload || {} }));
});
