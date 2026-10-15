//! CollabCraft transport for the browser build.
//!
//! The same jobs and events as the desktop worker ([`pdfcraft::cloud`]), but
//! the client is async and the session rides the same-origin cookie the service
//! set at sign-in. There is no device flow and no token file.
//!
//! ## Login and logout routing
//!
//! When the account check returns Unauthenticated the transport opens the
//! CollabCraft login page in a popup window.  The popup does OIDC and lands at
//! the service root `/` (the server always redirects there after the callback).
//! A polling closure watches for the popup to close, then stores the key
//! `"pdfcraft:auth_done"` in egui's ephemeral memory and requests a repaint.
//! The update loop sees the flag, clears it, and calls `sign_in()` to force a
//! fresh `Job::Account`.  If the popup is blocked by the browser the transport
//! falls back to a full-page redirect to `/auth/login`.
//!
//! Sign-out navigates to the service root `/` rather than reloading the
//! PDFCraft page.  Reloading would trigger the bootstrap account check, find
//! the session gone, and redirect to `/auth/login` immediately — a sign-out
//! loop that `start()`-in-`update()` would otherwise create.

use craft_cloud_client::{Client, Error};
use pdfcraft_ui_egui::cloud::{Account, CloudBackend, Event, Fault, Job, Post};
use wasm_bindgen::JsCast as _;
use wasm_bindgen_futures::spawn_local;

/// Hands cloud jobs to async tasks. Cheap to clone.
#[derive(Clone)]
pub struct Service {
    client: Client,
    ctx: egui::Context,
}

impl Service {
    /// Always available when the app is served by CollabCraft on the same
    /// origin. The page names the service; nothing is compiled in.
    pub fn new(ctx: egui::Context) -> Service {
        log::info!("cloud storage through {}", Client::from_page().base());
        Service { client: Client::from_page(), ctx }
    }
}

impl CloudBackend for Service {
    fn submit(&self, job: Job, post: Post) {
        let worker = self.clone();
        spawn_local(async move {
            worker.run(job, post).await;
        });
    }
}

impl Service {
    async fn run(self, job: Job, post: Post) {
        let post = |event: Event| {
            post.send(event);
            self.ctx.request_repaint();
        };
        match job {
            Job::Account => {
                let answer =
                    self.me().await.map(|me| Account { display: me.display, capabilities: me.capabilities, provider_linked: me.provider_linked });
                if matches!(&answer, Err(Fault::Unauthenticated)) {
                    offer_login_popup(&self.ctx);
                }
                post(Event::Account(answer));
            }
            Job::LinkStart => match self.client.link_start().await.map_err(fault) {
                Ok(start) => post(Event::Browser { url: start.login_url, handle: Some(start.handle) }),
                Err(f) => post(Event::Linked(Err(f))),
            },
            Job::LinkPoll(handle) => {
                let answer = self.client.link_poll(&handle).await.map_err(fault);
                post(Event::Linked(answer));
            }
            Job::SignOut => {
                let _ = fetch_logout().await;
                // Navigate to the service root rather than reloading the PDFCraft page.
                // Reloading would run the bootstrap account check, find the session gone,
                // and call offer_login_popup() — a sign-out loop.  The root page is a safe
                // landing without automatic cloud-credential checks.
                if let Some(window) = web_sys::window() {
                    let _ = window.location().assign("/");
                }
            }
            Job::List(path) => {
                let entries = self.client.list(&path).await.map_err(fault);
                post(Event::Listed { path, entries });
            }
            Job::SharedWithMe => {
                let answer = self.client.shared_with_me().await.map_err(fault);
                post(Event::Shared(answer));
            }
            Job::Read { entry, into } => {
                let id = entry.id.clone();
                let bytes = self.client.read(&id).await.map_err(fault);
                post(Event::Read { entry, into, bytes });
            }
            Job::Write { uid, entry, bytes } => {
                let id = entry.id.clone();
                let etag = entry.etag.clone();
                let answer = self.client.write(&id, &bytes, etag.as_deref()).await.map_err(fault);
                post(Event::Saved { uid, entry: answer });
            }
            Job::Create { uid, folder, name, bytes } => {
                let answer = self.client.create(&folder, &name, &bytes).await.map_err(fault);
                post(Event::Saved { uid, entry: answer });
            }
            Job::Conversation(id) => {
                let answer = self.client.conversation(&id).await.map_err(fault);
                post(Event::Conversation(answer));
            }
        }
    }

    async fn me(&self) -> Result<craft_cloud_client::Me, Fault> {
        self.client.me().await.map_err(fault)
    }
}

fn fault(e: Error) -> Fault {
    match e {
        Error::Unauthenticated => Fault::Unauthenticated,
        Error::NotLinked => Fault::NotLinked,
        Error::Unreachable(detail) => Fault::Unreachable(detail),
        Error::Refused { code, .. } if code == "conflict" || code == "409" || code == "412" => Fault::Conflict,
        Error::Refused { code, detail } => Fault::Refused(if detail.trim().is_empty() { code } else { detail }),
        Error::Malformed(detail) => Fault::Refused(format!("the cloud service gave an answer PdfCraft could not read: {detail}")),
        Error::Unsendable(detail) => {
            log::error!("cloud request could not be encoded: {detail}");
            Fault::Refused("PdfCraft could not build that request".to_owned())
        }
    }
}

/// Open the CollabCraft login page in a popup so the user returns to PDFCraft
/// after authenticating.  The CollabCraft OIDC callback always redirects to `/`
/// (server limitation, pinned rev 80bfaec); keeping the login in a child window
/// avoids losing the PDFCraft page.
///
/// A `setInterval` closure watches the popup and, when it closes, stores the
/// key `"pdfcraft:auth_done"` in egui's ephemeral memory and requests a repaint.
/// The update loop consumes this flag, sets `"pdfcraft:popup_retry"` to suppress
/// a second popup on the post-close account refresh, then calls `sign_in()` to
/// force a fresh `Job::Account`.
///
/// If the popup is blocked the function falls back to a full-page redirect to
/// `/auth/login`.  The user lands at `/` after OIDC and navigates back manually.
///
/// No popup is opened when:
/// - the current path starts with `/auth/` (the login page itself),
/// - this call is a post-popup-close account refresh (`"pdfcraft:popup_retry"`
///   key in egui memory), indicating the user cancelled the popup without
///   signing in.
fn offer_login_popup(ctx: &egui::Context) {
    let Some(window) = web_sys::window() else { return };
    let Ok(path) = window.location().pathname() else { return };
    // Already on the auth flow — don't recurse.
    if path.starts_with("/auth/") {
        return;
    }
    // Post-popup-close retry: the popup already ran one account check after
    // closing.  If the user cancelled (didn't log in), show the "Sign in"
    // badge instead of reopening the popup immediately.
    let is_retry = ctx.memory(|m| m.data.get_temp::<bool>(egui::Id::new("pdfcraft:popup_retry")).unwrap_or(false));
    if is_retry {
        ctx.memory_mut(|m| m.data.remove::<bool>(egui::Id::new("pdfcraft:popup_retry")));
        return;
    }

    // Try to open a popup.  Returns Ok(None) when blocked by the browser.
    let popup_result = window.open_with_url_and_target_and_features(
        "/auth/login",
        "_blank",
        "popup,width=520,height=620,menubar=no,toolbar=no,location=no",
    );

    match popup_result {
        Ok(Some(popup)) => {
            // Poll every 750 ms until the popup window closes.  On close, signal
            // the update loop via egui's ephemeral memory.
            let ctx = ctx.clone();
            let cb = wasm_bindgen::closure::Closure::<dyn FnMut()>::new(move || {
                // Window::closed() returns Result<bool, JsValue>; treat errors as closed.
                if popup.closed().unwrap_or(true) {
                    ctx.memory_mut(|m| {
                        m.data.insert_temp(egui::Id::new("pdfcraft:auth_done"), true);
                    });
                    ctx.request_repaint();
                    // The interval keeps firing but becomes a no-op once
                    // the update loop consumes "pdfcraft:auth_done".
                }
            });
            let _ = window.set_interval_with_callback_and_timeout_and_arguments_0(
                cb.as_ref().unchecked_ref(),
                750,
            );
            cb.forget();
        }
        _ => {
            // Popup blocked or unsupported.  Fall back to a full-page redirect.
            // The user will land at / after the OIDC callback and can navigate
            // back to PDFCraft from there.
            let _ = window.location().set_href("/auth/login");
        }
    }
}

async fn fetch_logout() -> Result<(), String> {
    let global = js_sys::global();
    let fetch = js_sys::Reflect::get(&global, &wasm_bindgen::JsValue::from_str("fetch"))
        .ok()
        .and_then(|v| v.dyn_into::<js_sys::Function>().ok())
        .ok_or_else(|| "no fetch".to_owned())?;
    let init = web_sys::RequestInit::new();
    init.set_method("POST");
    init.set_credentials(web_sys::RequestCredentials::Include);
    let req = web_sys::Request::new_with_str_and_init("/auth/logout", &init).map_err(|e| format!("{e:?}"))?;
    let promised = fetch.call1(global.as_ref(), req.as_ref()).map_err(|e| format!("{e:?}"))?;
    let promised: js_sys::Promise = promised.dyn_into().map_err(|_| "not a promise".to_owned())?;
    wasm_bindgen_futures::JsFuture::from(promised).await.map_err(|e| format!("{e:?}"))?;
    Ok(())
}
