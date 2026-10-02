# Web push notifications

A push notification reaches a user whose page of the app is closed: "your video is ready". The browser subscribes once, the app keeps its subscription, and when something happens the app posts an encrypted message to the browser's push service (Google's for Chrome, Apple's for Safari, Mozilla's for Firefox), which wakes the app's service worker to show it. Sending is free: a subrequest from the Worker to the push service.

## Set it up

```sh
ocre g pwa     # the manifest and service worker that shows the messages
ocre g push
ocre migrate
```

```text
  create  src/push.rs
  create  public/push.js
  create  tests/push.rs
  create  migrations/0002_create_push_subscriptions.sql
  update  templates/layout.html
  update  Cargo.toml
  update  .dev.vars
  update  src/lib.rs
```

- `src/push.rs` answers `GET /push/key` (the public VAPID key), `POST /push/subscriptions` (saves a browser's subscription; the same browser again updates it) and `POST /push/subscriptions/delete`, and sends with `notify_all(&ctx, &message)`, or `notify_user(&ctx, user_id, &message)` when the app has `ocre g auth` (subscriptions then carry the signed-in user's id).
- `public/push.js`, loaded by the layout, subscribes the browser when an element with `data-push-subscribe` is clicked (browsers only ask for permission after a click): `<button data-push-subscribe>Notify me</button>`. It fires `push:subscribed`, or `push:error` (whose default is an alert).
- The table `push_subscriptions` holds each browser's endpoint and keys.
- `.dev.vars` gets a VAPID key pair (`VAPID_PUBLIC_KEY`, `VAPID_PRIVATE_KEY`) and `VAPID_SUBJECT`, the contact push services may write to (`mailto:` or `https:`).
- `Cargo.toml` turns on Ocre's `push` feature (P-256 for the encryption and the signatures).

## Send

```rust
let message = ocre::push::message("Your video is ready", "Download it within 7 days.", "/videos/Xq3v9L");
let delivered = crate::push::notify_user(&ctx, user.id, &message).await?;
```

`ocre::push::message(title, body, path)` is the JSON the service worker of `ocre g pwa` shows; a click on the notification opens `path`. For each subscription, `ocre::push::send` encrypts the message for that browser (`aes128gcm`, RFC 8291), signs the request with the VAPID private key (RFC 8292, a 12-hour ES256 token) and posts it; the push service keeps it up to a day while the browser is offline. A subscription the push service answers 404 or 410 to is gone (the user revoked the permission or the browser dropped it): the generated code deletes it. One subrequest per browser, so `notify_all` sends to 40 at most per call; for more, send from a [job](jobs.md), a batch per run. Messages are at most 4,079 bytes.

## Browsers

Chrome, Edge and Firefox on desktop and Android accept subscriptions from any page served over HTTPS (or `localhost`). Safari on macOS does too; on iPhone and iPad (iOS 16.4 and later) only once the user has added the site to the Home Screen, which the manifest of `ocre g pwa` allows. The flow was checked in `ocre dev` against a local stand-in for a push service (the request's headers and encrypted body, and a 410 deleting the subscription); the encryption matches the example of RFC 8291. Delivery through Google's and Apple's push services has not been run by Ocre's tests (October 2026).

## Production

```sh
ocre push-keys >> .prod.vars        # a new key pair for production
ocre secrets push VAPID_PRIVATE_KEY --file .prod.vars
```

Then set `VAPID_PUBLIC_KEY` (the public half from `.prod.vars`) and `VAPID_SUBJECT` in `worker.env` of `cloudflare.config.ts`. Changing the key pair later invalidates every subscription: browsers must subscribe again.

## See also

- [Generators: `ocre g push`](../reference/generators.md#ocre-g-push) and [`ocre g pwa`](../reference/generators.md#ocre-g-pwa).
- [Background jobs](jobs.md) for sending to many subscribers.
