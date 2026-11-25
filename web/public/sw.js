self.addEventListener("install", () => self.skipWaiting());
self.addEventListener("activate", (event) => event.waitUntil(self.clients.claim()));

self.addEventListener("push", (event) => {
  let payload = { title: "TokenPulse", body: "A quota window was restored" };
  if (event.data) {
    try {
      const data = event.data.json();
      payload = { title: data.title || payload.title, body: data.body || payload.body };
    } catch {
      payload.body = event.data.text() || payload.body;
    }
  }
  event.waitUntil(self.registration.showNotification(payload.title, {
    body: payload.body,
    icon: "/icon-192.png",
    badge: "/icon-192.png",
  }));
});

self.addEventListener("notificationclick", (event) => {
  event.notification.close();
  event.waitUntil(self.clients.matchAll({ type: "window", includeUncontrolled: true }).then((clients) => {
    if (clients.length) return clients[0].focus();
    return self.clients.openWindow("/");
  }));
});
