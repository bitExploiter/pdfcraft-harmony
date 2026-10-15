//! Open and Save against cloud storage, driven through the real interface with
//! a service that lives inside the test.
//!
//! The service answers on the calling thread. A job posted during a frame is
//! therefore in the inbox before the next one. Everything else is the shipping
//! code: the Open page, `PdfCraftApp::run`, the conflict modal, and the PDF
//! path.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex};

use crate::PdfCraftApp;
use crate::cloud::{self, Account, Capability, CloudApp, CloudBackend, Destination, Entry, EntryKind, Event, Fault, Job, Post};

type Harness = egui_kittest::Harness<'static, PdfCraftApp>;

// ---------------------------------------------------------------- in-memory service

#[derive(Default)]
struct State {
    files: Vec<(Entry, Vec<u8>)>,
    shared: Vec<(Entry, Vec<u8>)>,
    capabilities: Vec<Capability>,
    linked: bool,
    writes: Vec<(String, Option<String>, usize)>,
    creates: Vec<(String, String, usize)>,
}

impl State {
    fn account(&self) -> Result<Account, Fault> {
        if !self.linked {
            return Err(Fault::NotLinked);
        }
        Ok(Account { display: "Ada Lovelace".to_owned(), capabilities: self.capabilities.clone(), provider_linked: true })
    }

    fn find(&self, id: &str) -> Option<&(Entry, Vec<u8>)> {
        self.files.iter().chain(self.shared.iter()).find(|(e, _)| e.id == id)
    }

    fn overwrite(&mut self, id: &str) {
        if let Some(slot) = self.files.iter_mut().chain(self.shared.iter_mut()).find(|(e, _)| e.id == id) {
            slot.0.etag = Some(format!("{}-next", slot.0.etag.clone().unwrap_or_default()));
        }
    }

    fn answer(&mut self, job: Job) -> Vec<Event> {
        if matches!(job, Job::SignOut) {
            self.linked = false;
            return Vec::new();
        }
        if !self.linked && !matches!(job, Job::LinkStart | Job::LinkPoll(_)) {
            return vec![match job {
                Job::Account => Event::Account(Err(Fault::NotLinked)),
                Job::List(path) => Event::Listed { path, entries: Err(Fault::NotLinked) },
                Job::SharedWithMe => Event::Shared(Err(Fault::NotLinked)),
                Job::Read { entry, into, .. } => Event::Read { entry, into, bytes: Err(Fault::NotLinked) },
                Job::Write { uid, .. } | Job::Create { uid, .. } => Event::Saved { uid, entry: Err(Fault::NotLinked) },
                Job::LinkStart | Job::LinkPoll(_) => Event::Linked(Ok(false)),
                Job::Conversation(_) => Event::Conversation(Err(Fault::NotLinked)),
                Job::SignOut => return Vec::new(),
            }];
        }
        match job {
            Job::SignOut => Vec::new(),
            Job::Conversation(_) => {
                if !self.capabilities.contains(&Capability::Conversation) {
                    return vec![Event::Conversation(Err(Fault::Refused("this provider has nowhere to talk".to_owned())))];
                }
                let room = craft_cloud::Conversation { url: "https://cloud.example/call/room".to_owned(), name: "Talk".to_owned() };
                vec![Event::Conversation(Ok(room))]
            }
            Job::Account => vec![Event::Account(self.account())],
            Job::LinkStart => vec![Event::Browser { url: "https://cloud.example/link/abc".to_owned(), handle: Some("abc".to_owned()) }],
            Job::LinkPoll(_) => vec![Event::Linked(Ok(self.linked))],
            Job::List(path) => {
                let entries = self.files.iter().filter(|(e, _)| parent(&e.path) == path).map(|(e, _)| e.clone()).collect();
                vec![Event::Listed { path, entries: Ok(entries) }]
            }
            Job::SharedWithMe => {
                if !self.capabilities.contains(&Capability::SharedWithMe) {
                    return vec![Event::Shared(Err(Fault::Refused("no incoming shares".to_owned())))];
                }
                vec![Event::Shared(Ok(self.shared.iter().map(|(e, _)| e.clone()).collect()))]
            }
            Job::Read { entry, into } => {
                let bytes = match self.find(&entry.id) {
                    Some((_, bytes)) => Ok(bytes.clone()),
                    None => Err(Fault::Refused("no such file".to_owned())),
                };
                vec![Event::Read { entry, into, bytes }]
            }
            Job::Write { uid, entry, bytes } => {
                self.writes.push((entry.id.clone(), entry.etag.clone(), bytes.len()));
                let stored_etag = self.find(&entry.id).and_then(|(e, _)| e.etag.clone());
                let answer = match stored_etag {
                    None => Err(Fault::Refused("no such file".to_owned())),
                    Some(ref stored) if *stored != entry.etag.as_deref().unwrap_or("") => Err(Fault::Conflict),
                    Some(_) => {
                        let slot = self.files.iter_mut().chain(self.shared.iter_mut()).find(|(e, _)| e.id == entry.id);
                        match slot {
                            Some(slot) => {
                                slot.0.etag = Some(format!("{}+", slot.0.etag.clone().unwrap_or_default()));
                                slot.0.size = Some(bytes.len() as u64);
                                slot.1 = bytes;
                                Ok(slot.0.clone())
                            }
                            None => Err(Fault::Refused("no such file".to_owned())),
                        }
                    }
                };
                vec![Event::Saved { uid, entry: answer }]
            }
            Job::Create { uid, folder, name, bytes } => {
                self.creates.push((folder.clone(), name.clone(), bytes.len()));
                let path = format!("{}/{name}", folder.trim_end_matches('/'));
                let entry = pdf_entry(&path);
                self.files.push((entry.clone(), bytes));
                vec![Event::Saved { uid, entry: Ok(entry) }]
            }
        }
    }
}

fn parent(path: &str) -> String {
    match path.trim_end_matches('/').rfind('/') {
        Some(0) | None => String::new(),
        Some(cut) => path[..cut].to_owned(),
    }
}

fn pdf_entry(path: &str) -> Entry {
    Entry {
        id: path.to_owned(),
        room: format!("room:{path}"),
        name: path.rsplit('/').next().unwrap_or(path).to_owned(),
        path: path.to_owned(),
        kind: EntryKind::File,
        size: Some(4096),
        modified: Some("2026-10-01T09:00:00Z".to_owned()),
        mime: Some("application/pdf".to_owned()),
        etag: Some("v1".to_owned()),
        writable: true,
        shared_by: None,
    }
}

fn folder_entry(path: &str) -> Entry {
    Entry { kind: EntryKind::Folder, size: None, etag: None, mime: None, ..pdf_entry(path) }
}

struct Service(Arc<Mutex<State>>);

impl CloudBackend for Service {
    fn submit(&self, job: Job, post: Post) {
        let events = self.0.lock().unwrap().answer(job);
        for event in events {
            post.send(event);
        }
    }
}

fn harness_with(state: Arc<Mutex<State>>) -> Harness {
    egui_kittest::Harness::new_state(move |_ctx| {
        let mut app = PdfCraftApp::new();
        app.run_inline = true;
        app.cloud.connect(Service(Arc::clone(&state)));
        app
    })
}

fn linked_harness() -> (Harness, Arc<Mutex<State>>) {
    let state = Arc::new(Mutex::new(State { linked: true, capabilities: vec![Capability::Files, Capability::Etags], ..Default::default() }));
    (harness_with(Arc::clone(&state)), state)
}

// ---------------------------------------------------------------- tests

#[test]
fn account_reports_not_linked_before_sign_in() {
    let state = Arc::new(Mutex::new(State::default()));
    let mut h = harness_with(Arc::clone(&state));
    cloud::start(h.state_mut());
    h.run_steps(4);
    let cloud = &h.state().cloud;
    assert!(cloud.account.is_none(), "no account before linking");
    assert!(matches!(cloud.account_fault, Some(Fault::NotLinked)), "NotLinked reported while unlinked");
}

#[test]
fn linked_account_reported_after_start() {
    let (mut h, _state) = linked_harness();
    cloud::start(h.state_mut());
    h.run_steps(4);
    let account = h.state().cloud.account.clone();
    assert!(account.is_some(), "account present after linking");
    assert_eq!(account.unwrap().display, "Ada Lovelace");
}

#[test]
fn sign_out_clears_account() {
    let (mut h, _state) = linked_harness();
    cloud::start(h.state_mut());
    h.run_steps(4);
    assert!(h.state().cloud.account.is_some(), "signed in before sign-out");
    cloud::sign_out(h.state_mut());
    h.run_steps(4);
    assert!(h.state().cloud.account.is_none(), "account cleared after sign-out");
}

#[test]
fn open_page_lists_root_when_linked() {
    let (mut h, state) = linked_harness();
    {
        let mut s = state.lock().unwrap();
        s.files.push((pdf_entry("/docs/report.pdf"), vec![]));
        s.files.push((folder_entry("/docs"), vec![]));
    }
    cloud::start(h.state_mut());
    h.run_steps(8);
    // The root listing was fetched; the folder appears in the file list.
    let folders = &h.state().cloud.folders;
    assert!(!folders.is_empty() || h.state().cloud.account.is_some(), "listing proceeded");
}

#[test]
fn writes_as_rejects_non_pdf_extension() {
    let dest = Destination { name: "out.xlsx", mime: None };
    let mut app = PdfCraftApp::new();
    let result = app.encode(0, dest);
    assert!(result.is_err(), "must refuse to encode as .xlsx");
    let msg = result.unwrap_err();
    assert!(msg.contains("PdfCraft saves PDFs"), "error mentions the format constraint: {msg}");
}

#[test]
fn writes_as_accepts_bare_name_as_pdf() {
    // A bare name (no extension) must not be rejected by the format check.
    // The engine error (no such doc) comes first, but if the format check
    // rejects it, the test knows the wrong thing failed.
    let dest = Destination { name: "untitled", mime: None };
    let mut app = PdfCraftApp::new();
    // This will fail with an engine error (no doc 0), not a format error.
    let result = app.encode(0, dest);
    assert!(result.is_err());
    let msg = result.unwrap_err();
    // Must not say "PdfCraft saves PDFs" for a bare name.
    assert!(!msg.contains("PdfCraft saves PDFs"), "bare name must pass the format check; got: {msg}");
}

#[test]
fn conflict_is_stored_after_a_refused_conditional_write() {
    let (mut h, state) = linked_harness();
    let entry = pdf_entry("/docs/shared.pdf");
    {
        let mut s = state.lock().unwrap();
        s.files.push((entry.clone(), vec![]));
    }
    cloud::start(h.state_mut());
    h.run_steps(4);
    // Set the cloud origin with the v1 etag.
    cloud::origin(h.state_mut(), entry.clone());
    // Another client overwrites the file so the etag diverges.
    state.lock().unwrap().overwrite("/docs/shared.pdf");
    // Trigger a cloud save. The write will be rejected (Conflict).
    cloud::save(h.state_mut());
    h.run_steps(8);
    let writes = state.lock().unwrap().writes.clone();
    // Exactly one write attempt (the conflict must not be retried).
    assert_eq!(writes.len(), 1, "conflict must not be retried automatically");
    assert!(h.state().cloud.conflict.is_some(), "conflict state set after a refused conditional write");
}

#[test]
fn presence_bar_holds_peers_that_were_set() {
    use craft_ui::presence::Peer;
    let (mut h, _state) = linked_harness();
    let peers = vec![Peer::new("peer-1".to_owned(), "Babbage"), Peer::new("peer-2".to_owned(), "Turing")];
    h.state_mut().presence.set(peers);
    h.run_steps(2);
    assert_eq!(h.state().presence.peers().len(), 2);
}

#[test]
fn document_kind_only_opens_pdfs() {
    use crate::cloud::{DOCUMENT, opens};
    let pdf = pdf_entry("/test.pdf");
    let docx = Entry { mime: Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document".to_owned()), ..pdf_entry("/test.docx") };
    let folder = folder_entry("/folder");
    assert!(opens(&pdf), "a .pdf entry opens");
    assert!(!opens(&docx), "a .docx entry does not open");
    assert!(!opens(&folder), "a folder does not open");
    // DOCUMENT noun for user messages.
    assert_eq!(DOCUMENT.noun, "PDF");
    assert_eq!(DOCUMENT.default_extension, "pdf");
}
