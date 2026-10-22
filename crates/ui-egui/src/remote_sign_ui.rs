//! Remote signature (OTP) in the Sign dialog: digital IDs whose key a signing service holds
//! (Aruba ARSS, or a provider of the Cloud Signature Consortium API), each signature authorized
//! with a one-time password.
//!
//! Finding an account's certificates, asking for a code and signing each talk to the service, so
//! they run on a worker thread (inline in tests, [`PdfCraftApp::run_inline`]) while the dialog
//! says what is happening, and an answer is used only by the showing of the dialog that asked.
//! Sign asks where to save the signed file first, as for any digital ID: the service spends the
//! code, and may count or bill the signature, only once the user has chosen.
//! The network is [`PdfCraftApp::remote_transport`], which only the desktop app supplies: this
//! crate has no network code, and without a transport (the web build, tests that don't set one)
//! remote IDs are greyed. The service password, the PIN and the code live in the dialog for one
//! signature and are never stored; an ID keeps only its `remote:` reference and what its
//! signatures ask for.

use std::path::PathBuf;
use std::sync::{Arc, mpsc};

use pdfcraft_engine::sign::remote::{self, Account, AuthNeeds, Confirmation, HttpTransport, OtpKind, OtpSent, Protocol, RemoteIdentity, Secrets};
use pdfcraft_engine::{DocId, SignJob};

use crate::sign_ui::{NewIdDraft, SignDraft, SignGraphic, SignRequest, SignStep, entry_for};
use crate::{Dialog, PdfCraftApp, widgets};

/// Why a remote ID can't be used here: the build has no network access for it.
const NOT_AVAILABLE: &str = "Remote signing isn't available in this build";

/// The sign crate's sentences the Sign dialog says in the interface language: the refusals
/// people meet most ([`remote::WORDINGS`]), and an Aruba account saved without its domain.
/// `the_service_sentences_are_in_every_catalog` keeps them in the catalogs.
fn service_words() -> impl Iterator<Item = &'static str> {
    remote::WORDINGS.iter().copied().chain([remote::NEEDS_DOMAIN])
}

/// A Sign dialog error in the interface language: a catalog key as such, and a signing service's
/// refusal as its sentence ([`service_words`], or [`remote::UNREACHABLE`]) with what the service
/// or the network adds after it as it came. Anything else is shown as the layer below words it.
/// Errors are kept in English and said here, on the UI thread: the catalog language is per
/// thread, and a request's worker has none.
pub(crate) fn said(e: &str) -> String {
    if crate::i18n::has(crate::i18n::current(), e) {
        return tl!(e).to_string();
    }
    // Only a whole sentence: "rejected the PIN" also begins "rejected the PIN or the OTP".
    for words in service_words() {
        if let Some(rest) = e.strip_prefix(words)
            && (rest.is_empty() || rest.starts_with(": ") || rest.starts_with(" ("))
        {
            return format!("{}{rest}", tl!(words));
        }
    }
    match e.strip_prefix(remote::UNREACHABLE).and_then(|r| r.strip_prefix(" (")).and_then(|r| r.strip_suffix(')')) {
        Some(reason) => crate::i18n::fmt(tl!("the signing service is unreachable ({reason})"), &[("reason", reason)]),
        None => e.to_string(),
    }
}

/// How a secret a draft holds prints in `Debug`: whether one was typed, never what (as the sign
/// crate's [`Secrets`] prints none).
pub(crate) struct Redacted<'a>(pub(crate) &'a str);

impl std::fmt::Debug for Redacted<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_empty() { "\"\"" } else { "<redacted>" })
    }
}

/// What a remote ID's signatures ask for besides the service password, as its service listed it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RemoteNeeds {
    /// A static PIN.
    pub pin: bool,
    pub otp: RemoteOtp,
    /// Each signature is confirmed on the user's other device (a phone app) while it is made,
    /// which the dialog says while it waits. Settings without it confirm nothing.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub confirm: bool,
    /// ... but not one made with the code: the confirmation is the code's alternative, used
    /// only when none is typed. Settings without it confirm with the code too.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unless_code: bool,
}

/// How the user gets the one-time password (the sign crate's [`OtpKind`], as settings keep it).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RemoteOtp {
    None,
    /// Read from an authenticator app or a token.
    Typed,
    /// The service sends it (an SMS) when asked: Send code.
    Sent,
}

impl RemoteNeeds {
    /// A remote ID whose needs weren't kept, or can't be read (settings edited by hand, or
    /// written by a later version that knows more), asks for everything: a field left empty is
    /// simply not sent.
    pub(crate) const UNKNOWN: Self = Self { pin: true, otp: RemoteOtp::Sent, confirm: false, unless_code: false };

    fn of(a: &AuthNeeds) -> Self {
        let otp = match a.otp {
            OtpKind::None => RemoteOtp::None,
            OtpKind::Typed => RemoteOtp::Typed,
            OtpKind::Sent => RemoteOtp::Sent,
        };
        Self { pin: a.pin, otp, confirm: a.confirm != Confirmation::Never, unless_code: a.confirm == Confirmation::WithoutCode }
    }

    /// Whether a signature made with a code typed (`code`) or without one is confirmed on the
    /// user's other device while the service waits.
    pub(crate) fn confirms(self, code: bool) -> bool {
        self.confirm && !(code && self.unless_code)
    }
}

/// A saved remote ID's needs, read leniently: a value this version can't read is
/// [`RemoteNeeds::UNKNOWN`], so it never takes the whole list of digital IDs down with it.
pub(crate) fn saved_needs<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<RemoteNeeds>, D::Error> {
    let v = <serde_json::Value as serde::Deserialize>::deserialize(d)?;
    Ok((!v.is_null()).then(|| serde_json::from_value(v).unwrap_or(RemoteNeeds::UNKNOWN)))
}

/// Configure a Digital ID ▸ Remote signature (OTP): the account to find certificates in.
#[derive(Clone, PartialEq)]
pub struct RemoteAccountDraft {
    pub service: Protocol,
    /// Aruba's endpoint, or the CSC provider's base address (ending in `/csc/v2`).
    pub address: String,
    pub user: String,
    /// Aruba: the domain ("dominio") that comes with the account. Aruba refuses an account
    /// without it, so Find certificates waits for it.
    pub domain: String,
    /// Signs in to list the certificates, and is never stored.
    pub password: String,
}

impl Default for RemoteAccountDraft {
    fn default() -> Self {
        Self { service: Protocol::Arss, address: remote::ARSS_ENDPOINT.into(), user: String::new(), domain: String::new(), password: String::new() }
    }
}

// By hand, so the password never prints.
impl std::fmt::Debug for RemoteAccountDraft {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self { service, address, user, domain, password } = self;
        f.debug_struct("RemoteAccountDraft")
            .field("service", service)
            .field("address", address)
            .field("user", user)
            .field("domain", domain)
            .field("password", &Redacted(password))
            .finish()
    }
}

impl RemoteAccountDraft {
    /// Whether Find certificates can ask: an Aruba account needs its domain.
    pub(crate) fn complete(&self) -> bool {
        self.service != Protocol::Arss || !self.domain.trim().is_empty()
    }

    fn account(&self) -> Account {
        let domain = self.domain.trim();
        Account {
            protocol: self.service,
            url: self.address.trim().to_string(),
            user: self.user.trim().to_string(),
            domain: (self.service == Protocol::Arss && !domain.is_empty()).then(|| domain.to_string()),
            hsm: None,
        }
    }

    /// The service was changed: Aruba's address is filled in for it, and taken away for a CSC
    /// provider, whose address the user must give. An address typed by hand stays.
    fn follow_service(&mut self) {
        match self.service {
            Protocol::Arss if self.address.trim().is_empty() => self.address = remote::ARSS_ENDPOINT.into(),
            Protocol::Csc if self.address == remote::ARSS_ENDPOINT => self.address.clear(),
            _ => {}
        }
    }
}

/// The request to the signing service the dialog is waiting for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteBusy {
    /// Find certificates.
    Finding,
    /// Send code.
    Sending,
    /// Sign, before the service is asked: the Save picker is showing (never in a browser).
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    Choosing,
    /// Sign.
    Signing,
}

/// What a finished request hands back.
enum Answer {
    Found(Vec<RemoteIdentity>),
    Sent(OtpSent),
    Signed(Arc<Vec<u8>>),
}

/// A document being signed by its service: its edit generation when the job was made (an edit
/// since isn't in the signed file), the graphic to remember, and the file chosen to save it to.
struct Signing {
    doc: DocId,
    generation: u64,
    choice: SignGraphic,
    path: PathBuf,
}

/// Sign was clicked for a remote ID: what the service signs and with what, kept until the user
/// has chosen where to save the signed file.
struct ToSign {
    transport: Arc<dyn HttpTransport>,
    account: Account,
    credential: String,
    secrets: Secrets,
    job: SignJob,
    doc: DocId,
    generation: u64,
    choice: SignGraphic,
}

/// A request running on its worker, or a signature waiting for the Save picker, and where its
/// answer goes. It goes when the dialog closes, and with it the secrets a waiting one holds.
pub(crate) struct RemoteTask {
    /// The showing of the Sign dialog that asked ([`PdfCraftApp::dialog_epoch`]).
    epoch: u64,
    state: TaskState,
}

enum TaskState {
    /// The Save picker is showing; nothing has gone to the service yet.
    #[cfg(not(target_arch = "wasm32"))]
    Choosing(Box<ToSign>),
    /// A request on its worker, which answers through `rx`.
    Running { busy: RemoteBusy, signing: Option<Signing>, rx: mpsc::Receiver<Result<Answer, String>> },
}

impl RemoteTask {
    fn busy(&self) -> RemoteBusy {
        match &self.state {
            #[cfg(not(target_arch = "wasm32"))]
            TaskState::Choosing(_) => RemoteBusy::Choosing,
            TaskState::Running { busy, .. } => *busy,
        }
    }
}

impl PdfCraftApp {
    /// The request the showing Sign dialog is waiting for, if any.
    pub(crate) fn remote_busy(&mut self) -> Option<RemoteBusy> {
        self.forget_cancelled_save();
        let epoch = self.dialog_epoch();
        self.remote_task.as_ref().filter(|t| t.epoch == epoch).map(RemoteTask::busy)
    }

    /// A signature waiting for a Save picker that is gone without a file (cancelled, or the
    /// document changed meanwhile) is dropped: nothing was signed, and the secrets go with it.
    fn forget_cancelled_save(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        if self.remote_task.as_ref().is_some_and(|t| matches!(t.state, TaskState::Choosing(_))) && !self.pickers.pending() {
            self.remote_task = None;
        }
    }

    fn transport(&self) -> Result<Arc<dyn HttpTransport>, String> {
        self.remote_transport.clone().ok_or_else(|| NOT_AVAILABLE.to_string())
    }

    /// Configure ▸ Remote signature ▸ Find certificates: sign in to the service and list the
    /// account's certificates that can sign, which then join the list of digital IDs.
    pub(crate) fn find_remote_ids(&mut self) -> Result<(), String> {
        let transport = self.transport()?;
        let draft = self.sign_draft.as_ref().map(|d| d.new_id.remote.clone()).ok_or("nothing to configure")?;
        let account = draft.account();
        let password = draft.password;
        self.run_remote(RemoteBusy::Finding, None, move || {
            remote::list(&*transport, &account, &password).map(Answer::Found).map_err(|e| e.to_string())
        })
    }

    /// Sign as ▸ Send code: ask the service to send the chosen remote ID's one-time password.
    pub(crate) fn send_remote_code(&mut self) -> Result<(), String> {
        let transport = self.transport()?;
        let d = self.sign_draft.as_ref().ok_or("nothing to sign")?;
        let entry = d.selected.and_then(|i| self.digital_ids.get(i)).filter(|e| e.is_remote()).ok_or("Choose a digital ID.")?;
        let (account, credential) = remote::parse_reference(&entry.path).map_err(|e| e.to_string())?;
        let password = d.password.clone();
        if let Some(d) = self.sign_draft.as_mut() {
            d.otp_note = None;
        }
        self.run_remote(RemoteBusy::Sending, None, move || {
            remote::send_otp(&*transport, &account, &credential, &password).map(Answer::Sent).map_err(|e| e.to_string())
        })
    }

    /// Sign as ▸ Sign with a remote ID: ask where to save the signed file, then the service signs
    /// the document as it was when Sign was clicked, on a worker thread, and the file is written
    /// there. A cancelled Save picker sends nothing to the service.
    pub(crate) fn start_remote_signing(&mut self) -> Result<(), String> {
        let transport = self.transport()?;
        // The Save picker shows first: never while another one is showing, which would refuse it.
        #[cfg(not(target_arch = "wasm32"))]
        if self.save_override.is_none() && self.pickers.pending() {
            return Err("Another file dialog is still open. Finish with it first.".into());
        }
        let SignRequest { doc, draft, entry, opts } = self.sign_request()?;
        let (account, credential) = remote::parse_reference(&entry.path).map_err(|e| e.to_string())?;
        let job = self.session.sign_job(doc, opts).map_err(|e| e.to_string())?;
        let generation = self.session.get(doc).map(|d| d.edit_generation()).ok_or("no document")?;
        let given = |s: &str| (!s.trim().is_empty()).then(|| s.trim().to_string());
        // Send code got the service to send it (or to show a challenge for it), for this ID.
        let otp_sent = draft.otp_note.is_some();
        let secrets = Secrets { password: draft.password.clone(), pin: given(&draft.pin), otp: given(&draft.otp), otp_sent };
        let to_sign = ToSign { transport, account, credential, secrets, job, doc, generation, choice: draft.graphic };
        match self.save_override.clone() {
            Some(path) => self.sign_remotely(to_sign, PathBuf::from(path)),
            #[cfg(not(target_arch = "wasm32"))]
            None => {
                let epoch = self.dialog_epoch();
                let dialog = self.signed_save_dialog(doc);
                if let Some(d) = self.sign_draft.as_mut() {
                    d.error = None;
                }
                self.remote_task = Some(RemoteTask { epoch, state: TaskState::Choosing(Box::new(to_sign)) });
                // The pick is dropped (with a notice) if the document is edited or switched away
                // from meanwhile, and then nothing is signed.
                let asked = self.ask_one(crate::pickers::Ask::Save(dialog), Some(doc), move |app, path| app.sign_into(epoch, path));
                if !asked {
                    self.remote_task = None;
                    return Err(String::new());
                }
                Ok(())
            }
            // A browser has no transport to get this far.
            #[cfg(target_arch = "wasm32")]
            None => {
                drop(to_sign);
                Err(String::new())
            }
        }
    }

    /// The Save picker answered with `path`: the service signs, if the showing of the dialog
    /// that asked is still open. One closed meanwhile took the signature's secrets with it.
    #[cfg(not(target_arch = "wasm32"))]
    fn sign_into(&mut self, epoch: u64, path: PathBuf) {
        let open = self.dialog == Some(Dialog::Sign) && self.sign_draft.is_some() && self.dialog_epoch() == epoch;
        let Some(task) = self.remote_task.take_if(|t| open && t.epoch == epoch && matches!(t.state, TaskState::Choosing(_))) else { return };
        let TaskState::Choosing(to_sign) = task.state else { return };
        if let Err(e) = self.sign_remotely(*to_sign, path)
            && let Some(d) = self.sign_draft.as_mut()
        {
            d.error = Some(e);
        }
    }

    /// The service signs `to_sign` on a worker thread, and the signed file is written to `path`.
    fn sign_remotely(&mut self, to_sign: ToSign, path: PathBuf) -> Result<(), String> {
        let ToSign { transport, account, credential, secrets, job, doc, generation, choice } = to_sign;
        self.run_remote(RemoteBusy::Signing, Some(Signing { doc, generation, choice, path }), move || {
            let id = remote::digital_id(transport, &account, &credential, secrets).map_err(|e| e.to_string())?;
            job.run(&id).map(Answer::Signed).map_err(|e| e.to_string())
        })
    }

    /// Run `work` on a worker thread for the showing Sign dialog, which waits for its answer.
    fn run_remote(
        &mut self,
        busy: RemoteBusy,
        signing: Option<Signing>,
        work: impl FnOnce() -> Result<Answer, String> + Send + 'static,
    ) -> Result<(), String> {
        if self.remote_busy().is_some() {
            // The buttons are disabled meanwhile; a repeat (Enter in a field) waits for the first.
            return Err(String::new());
        }
        let (tx, rx) = mpsc::channel();
        let ctx = self.ctx.clone();
        let job = move || {
            // A panic in the protocol code comes back as an error, so the dialog never waits forever.
            let answer = pdfcraft_engine::guard(work).unwrap_or_else(|m| Err(format!("That didn't work: an internal error stopped it ({m}).")));
            // The receiver is gone when the dialog was opened again since, or the app quit:
            // nobody to answer then.
            let _ = tx.send(answer);
            if let Some(ctx) = ctx {
                ctx.request_repaint();
            }
        };
        let epoch = self.dialog_epoch();
        if let Some(d) = self.sign_draft.as_mut() {
            d.error = None;
        }
        self.remote_task = Some(RemoteTask { epoch, state: TaskState::Running { busy, signing, rx } });
        #[cfg(not(target_arch = "wasm32"))]
        if self.run_inline {
            job();
        } else if let Err(e) = std::thread::Builder::new().name("pdfcraft-remote-sign".into()).spawn(job) {
            // No worker: say so instead of waiting for an answer that never comes.
            self.remote_task = None;
            return Err(e.to_string());
        }
        // A browser has no transport to get this far; it would run in place.
        #[cfg(target_arch = "wasm32")]
        job();
        self.poll_remote_sign();
        Ok(())
    }

    /// Use the answer of a finished request (each frame), in the showing of the Sign dialog that
    /// asked: one closed meanwhile, or opened again since, drops it.
    pub(crate) fn poll_remote_sign(&mut self) {
        self.forget_cancelled_save();
        let Some(RemoteTask { state: TaskState::Running { rx, .. }, .. }) = &self.remote_task else { return };
        let answer = match rx.try_recv() {
            Ok(answer) => answer,
            Err(mpsc::TryRecvError::Empty) => return,
            // The worker always answers, a panic included; this is one that never ran.
            Err(mpsc::TryRecvError::Disconnected) => Err("the request to the signing service stopped unexpectedly".into()),
        };
        let Some(RemoteTask { epoch, state: TaskState::Running { signing, .. } }) = self.remote_task.take() else { return };
        if epoch != self.dialog_epoch() || self.dialog != Some(Dialog::Sign) || self.sign_draft.is_none() {
            return;
        }
        match answer {
            Ok(Answer::Found(ids)) => {
                let first = self.add_remote_ids(&ids);
                let Some(d) = self.sign_draft.as_mut() else { return };
                match first {
                    Some(i) => {
                        d.selected = Some(i);
                        d.step = SignStep::Choose;
                        d.new_id = NewIdDraft::default();
                    }
                    None => d.error = Some(tl!("The signing service lists no certificate for this account.").to_string()),
                }
            }
            Ok(Answer::Sent(sent)) => {
                let Some(d) = self.sign_draft.as_mut() else { return };
                d.otp_note = Some(match sent {
                    OtpSent::Sent => tl!("Code sent").to_string(),
                    OtpSent::Challenge(c) => crate::i18n::fmt(tl!("Challenge: {challenge}"), &[("challenge", &c)]),
                });
            }
            Ok(Answer::Signed(signed)) => self.save_remote_signature(signing, signed),
            Err(e) => {
                if let Some(d) = self.sign_draft.as_mut() {
                    d.error = Some(e);
                }
            }
        }
    }

    /// Add (or refresh) a digital ID for each identity a service listed. Returns the first's index.
    fn add_remote_ids(&mut self, ids: &[RemoteIdentity]) -> Option<usize> {
        let mut first = None;
        for id in ids {
            let mut entry = entry_for(&id.reference(), &id.certificate);
            entry.remote = Some(RemoteNeeds::of(&id.auth));
            let i = match self.digital_ids.iter().position(|e| e.path == entry.path) {
                Some(i) => {
                    if let Some(slot) = self.digital_ids.get_mut(i) {
                        *slot = entry;
                    }
                    i
                }
                None => {
                    self.digital_ids.push(entry);
                    self.digital_ids.len() - 1
                }
            };
            first.get_or_insert(i);
        }
        first
    }

    /// Write the file a service signed where the user chose, as any signature is saved, and
    /// close the dialog. Not if the document was edited while it was being signed: those edits
    /// aren't in the signed file.
    fn save_remote_signature(&mut self, signing: Option<Signing>, signed: Arc<Vec<u8>>) {
        let Some(Signing { doc, generation, choice, path }) = signing else { return };
        let unchanged = self.active_ids().map(|(_, id)| id) == Some(doc) && self.session.get(doc).is_some_and(|d| d.edit_generation() == generation);
        let saved = if unchanged {
            self.write_signed(doc, choice, &path, signed)
        } else {
            Err(tl!("The document changed while it was being signed, so the signature wasn't saved.").to_string())
        };
        match saved {
            Ok(()) => {
                self.sign_draft = None;
                self.dialog = None;
            }
            Err(e) => {
                if let Some(d) = self.sign_draft.as_mut() {
                    d.error = Some(e);
                }
            }
        }
    }
}

/// Configure ▸ Remote signature (OTP): the service and the account. Returns whether Enter was
/// pressed in the password field (Find certificates, once the account is
/// [complete](RemoteAccountDraft::complete)).
pub(crate) fn account_fields(ui: &mut egui::Ui, a: &mut RemoteAccountDraft) -> bool {
    let mut submit = false;
    egui::Grid::new("remote-account").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
        ui.label(tl!("Service"));
        let before = a.service;
        ui.vertical(|ui| {
            // The service's own name: not translated.
            widgets::radio_value(ui, &mut a.service, Protocol::Arss, "Aruba (ARSS)");
            widgets::radio_value(ui, &mut a.service, Protocol::Csc, tl!("Other provider (CSC API)"));
        });
        if a.service != before {
            a.follow_service();
        }
        ui.end_row();
        let l = ui.label(tl!("Service address"));
        ui.add(widgets::line(&mut a.address).hint_text("https://…/csc/v2").desired_width(360.0)).labelled_by(l.id);
        ui.end_row();
        let l = ui.label(tl!("Username"));
        ui.add(widgets::line(&mut a.user).desired_width(220.0)).labelled_by(l.id);
        ui.end_row();
        if a.service == Protocol::Arss {
            let l = ui.label(tl!("Domain"));
            ui.add(widgets::line(&mut a.domain).hint_text(tl!("Required, given by Aruba with the account")).desired_width(360.0)).labelled_by(l.id);
            ui.end_row();
        }
        let l = ui.label(tl!("Password"));
        let r = ui.add(widgets::line(&mut a.password).password(true).desired_width(220.0)).labelled_by(l.id);
        submit = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        ui.end_row();
    });
    submit && a.complete()
}

/// Sign as for a remote ID, as rows of the Sign as grid: the service password, then the PIN and
/// the one-time code when its signatures ask for them, with Send code when the service sends the
/// code. Returns (Enter was pressed in one of them, Send code was clicked); neither while `busy`.
pub(crate) fn secret_rows(ui: &mut egui::Ui, d: &mut SignDraft, needs: RemoteNeeds, busy: bool) -> (bool, bool) {
    let enter = |ui: &egui::Ui, r: &egui::Response| r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    let mut submit = false;
    let mut send = false;
    let l = ui.label(tl!("Password"));
    let r = ui.add(widgets::line(&mut d.password).password(true).desired_width(200.0)).labelled_by(l.id);
    submit |= enter(ui, &r);
    ui.end_row();
    if needs.pin {
        let l = ui.label(tl!("PIN"));
        let r = ui.add(widgets::line(&mut d.pin).password(true).desired_width(140.0)).labelled_by(l.id);
        submit |= enter(ui, &r);
        ui.end_row();
    }
    if needs.otp != RemoteOtp::None {
        let l = ui.label(tl!("One-time code (OTP)"));
        ui.horizontal(|ui| {
            let r = ui.add(widgets::line(&mut d.otp).desired_width(140.0)).labelled_by(l.id);
            submit |= enter(ui, &r);
            if needs.otp == RemoteOtp::Sent && ui.add_enabled_ui(!busy, |ui| widgets::pill_button(ui, tl!("Send code"), false)).inner.clicked() {
                send = true;
            }
        });
        ui.end_row();
    }
    (submit && !busy, send && !busy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::Lang;
    use pdfcraft_engine::sign::remote::AuthRole;

    /// The languages whose catalogs the i18n tests hold complete, and Norwegian Bokmål, whose
    /// catalog isn't held complete yet but has every remote-signing label.
    const CATALOGS: [&str; 19] =
        ["ja", "it", "pt-br", "es", "cs", "tr", "zh-hans", "fr", "de", "ru", "lt", "vi", "sl", "bg", "hu", "kk", "uk", "ar", "nb"];

    /// The remote-signing labels Sign with a Digital ID shows outside this file.
    const DIALOG_LABELS: [&str; 10] = [
        "Signing service  ·  ",
        "Remote signing isn't available in this build",
        "Remove \"{name}\" from this list? The certificate stays with the signing service.",
        "Remote signature (OTP)",
        "Looking for certificates…",
        "Find certificates",
        "Sending the code…",
        "Choose where to save the signed file. The service signs it once you have.",
        "Signing… Confirm the signature on your other device.",
        "Signing…",
    ];

    #[test]
    fn the_service_sentences_are_in_every_catalog() {
        // The frame `said` fills is the sign crate's own sentence.
        assert_eq!(format!("{} ({{reason}})", remote::UNREACHABLE), "the signing service is unreachable ({reason})");
        let code = include_str!("remote_sign_ui.rs").split("#[cfg(test)]\nmod ").next().unwrap_or_default();
        let here = code.match_indices("tl!(\"").filter_map(|(at, m)| code.get(at + m.len()..)?.split_once("\")").map(|(label, _)| label));
        let labels: Vec<&str> = here.chain(DIALOG_LABELS).chain(service_words()).collect();
        assert!(labels.contains(&"Send code") && labels.contains(&remote::REJECTED_PIN_OR_OTP), "{labels:?}");
        for code in CATALOGS {
            let lang = Lang::from_code(code).expect("registered");
            let missing: Vec<_> = labels.iter().filter(|l| !crate::i18n::has(lang, l)).collect();
            assert!(missing.is_empty(), "{code} lacks {missing:#?}");
        }
    }

    #[test]
    fn a_refusal_is_said_whole_with_what_the_service_adds_as_it_came() {
        crate::i18n::set_current(Lang::from_code("it").expect("it"));
        assert_eq!(said("the signing service rejected the PIN"), "il servizio di firma ha rifiutato il PIN");
        assert_eq!(said("the signing service rejected the PIN: Invalid PIN"), "il servizio di firma ha rifiutato il PIN: Invalid PIN");
        // A sentence that begins a longer one is not taken for it.
        assert_eq!(said(remote::REJECTED_PIN_OR_OTP), "il servizio di firma ha rifiutato il PIN o l'OTP (l'OTP potrebbe essere scaduto)");
        assert_eq!(
            said(&format!("{}: Invalid PIN or OTP", remote::REJECTED_PIN_OR_OTP)),
            "il servizio di firma ha rifiutato il PIN o l'OTP (l'OTP potrebbe essere scaduto): Invalid PIN or OTP"
        );
        assert_eq!(said(remote::NEEDS_DOMAIN), tl!(remote::NEEDS_DOMAIN));
        assert_ne!(said(remote::NEEDS_DOMAIN), remote::NEEDS_DOMAIN);
        assert_eq!(said("the signing service refused to sign (code 0003)"), "il servizio di firma ha rifiutato di firmare (code 0003)");
        assert_eq!(said("the signing service is unreachable (timed out)"), "il servizio di firma non è raggiungibile (timed out)");
        // Only the whole sentence, not one that merely starts the same way.
        assert_eq!(said("the signing service refused to signal"), "the signing service refused to signal");
        assert_eq!(said("the signing service is unreachable"), "the signing service is unreachable");
        // The dialog's own keys, and the rest as they are.
        assert_eq!(said("Choose a digital ID."), tl!("Choose a digital ID."));
        assert_eq!(said("the signing service refused the request (HTTP 500)"), "the signing service refused the request (HTTP 500)");
        crate::i18n::set_current(Lang::EN);
        assert_eq!(said("the signing service rejected the PIN: Invalid PIN"), "the signing service rejected the PIN: Invalid PIN");
    }

    /// A headless app showing Sign as for the mock CSC service's P-256 key (a code from an app),
    /// with the password and the code typed and no file to save to yet.
    fn signing_as(service: &Arc<remote::mock::MockService>) -> PdfCraftApp {
        let mut app = PdfCraftApp::new();
        let bytes = app.session.create_blank(300.0, 400.0, 1).unwrap();
        app.open_bytes("blank.pdf", None, bytes.as_ref().clone()).unwrap();
        app.remote_transport = Some(service.clone());
        app.run_inline = true;
        app.digital_ids.push(crate::DigitalIdEntry {
            path: remote::reference(&service.account(), remote::mock::P256),
            name: "Mario Rossi".into(),
            issuer: String::new(),
            email: String::new(),
            expires: String::new(),
            unusable: None,
            remote: Some(RemoteNeeds { pin: false, otp: RemoteOtp::Typed, confirm: false, unless_code: false }),
        });
        app.start_signing(0, None, None, None);
        let d = app.sign_draft.as_mut().unwrap();
        (d.step, d.password, d.otp) = (SignStep::SignAs, remote::mock::PASSWORD.into(), service.current_otp());
        app
    }

    /// A fresh folder per call: tests run in parallel.
    fn scratch() -> PathBuf {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("pdfcraft-remote-sign-ui-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Where to save is asked before the service signs: a cancelled Save picker spends no code
    /// and makes no signature, and the same code then signs into the file chosen.
    #[test]
    fn nothing_goes_to_the_service_until_a_file_is_chosen() {
        let service = Arc::new(remote::mock::MockService::csc_v2().unwrap());
        let mut app = signing_as(&service);
        app.pick_override = Some(Vec::new());
        assert_eq!(app.start_remote_signing(), Ok(()));
        assert!(service.requests().is_empty(), "asked before the picker answered");
        assert_eq!(app.remote_busy(), Some(RemoteBusy::Choosing));
        app.process_picked();
        assert_eq!(app.remote_busy(), None, "cancelled: the dialog can be used again");
        assert!(app.remote_task.is_none(), "and the secrets the signature held are gone");
        assert!(service.requests().is_empty(), "nothing was signed: {:?}", service.requests().iter().map(|r| r.url.clone()).collect::<Vec<_>>());
        assert_eq!(app.dialog, Some(Dialog::Sign));

        let dir = scratch();
        let chosen = dir.join("chosen.pdf");
        app.pick_override = Some(vec![chosen.to_string_lossy().into_owned()]);
        assert_eq!(app.start_remote_signing(), Ok(()));
        assert!(service.requests().is_empty());
        app.process_picked();
        assert_eq!(app.dialog, None, "{:?}", app.sign_draft.as_ref().and_then(|d| d.error.clone()));
        assert!(service.requests().iter().any(|r| r.url.ends_with("signatures/signHash")));
        let doc = app.session.get(app.views[0].id).unwrap();
        assert!(doc.is_signed());
        assert_eq!(std::fs::read(&chosen).unwrap(), *doc.bytes);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The Sign dialog closed while the Save picker showed: the file chosen after is not signed.
    #[test]
    fn a_file_chosen_after_the_dialog_closed_is_not_signed() {
        let service = Arc::new(remote::mock::MockService::csc_v2().unwrap());
        let mut app = signing_as(&service);
        let dir = scratch();
        let chosen = dir.join("chosen.pdf");
        app.pick_override = Some(vec![chosen.to_string_lossy().into_owned()]);
        assert_eq!(app.start_remote_signing(), Ok(()));
        app.dialog = None;
        app.process_picked();
        assert!(service.requests().is_empty());
        assert!(!chosen.exists());
        assert!(!app.session.get(app.views[0].id).unwrap().is_signed());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_credential_confirmed_on_another_device_is_kept_so() {
        let object = |kind: &str, role| remote::AuthObject { id: kind.into(), kind: kind.into(), label: None, format: None, role };
        let phone =
            AuthNeeds { pin: false, otp: OtpKind::None, objects: vec![object("PasswordOOB", AuthRole::Confirm)], confirm: Confirmation::Always };
        let needs = RemoteNeeds::of(&phone);
        assert_eq!(needs, RemoteNeeds { pin: false, otp: RemoteOtp::None, confirm: true, unless_code: false });
        assert!(needs.confirms(false) && needs.confirms(true));
        let sms = AuthNeeds {
            pin: true,
            otp: OtpKind::Sent,
            objects: vec![object("PIN", AuthRole::Pin), object("OTP", AuthRole::Otp)],
            confirm: Confirmation::Never,
        };
        let needs = RemoteNeeds::of(&sms);
        assert_eq!(needs, RemoteNeeds { pin: true, otp: RemoteOtp::Sent, confirm: false, unless_code: false });
        assert!(!needs.confirms(false) && !needs.confirms(true));
        // A phone or an app's code: the phone only when no code is typed.
        let either = AuthNeeds {
            pin: false,
            otp: OtpKind::Typed,
            objects: vec![object("PasswordOOB", AuthRole::Confirm), object("APP", AuthRole::Otp)],
            confirm: Confirmation::WithoutCode,
        };
        let needs = RemoteNeeds::of(&either);
        assert_eq!(needs, RemoteNeeds { pin: false, otp: RemoteOtp::Typed, confirm: true, unless_code: true });
        assert!(needs.confirms(false) && !needs.confirms(true));
    }
}
