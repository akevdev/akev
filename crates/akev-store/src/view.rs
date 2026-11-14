//! Read-side helpers: the [`FilterTarget`] implementations over records, filtered scans and
//! filter-autocompletion context.
//!
//! Filters without body terms run directly on the records under the store's read lock
//! (cheap: summary and header fields only). Body terms may have to read spill files and
//! decompress, so those scans copy what the evaluator needs ([`OwnedRecord`]: summary,
//! headers, body references) under the lock in chunks, release it, and evaluate the copies.

use crate::record::{BodyRef, Record, StoredBody};
use crate::spill::Spill;
use crate::util;
use akev_core::{ExchangeId, ExchangeSummary, Headers};
use akev_filter::{Filter, FilterTarget, SuggestContext};
use std::borrow::Cow;
use std::cell::OnceCell;
use std::collections::{BTreeMap, HashMap};

/// Decompression cap when decoding bodies for body filters.
const FILTER_MAX_DECODE: usize = 64 * 1024 * 1024;
/// Body-filter chunks of at least this many records run on several threads.
const PARALLEL_MIN_RECORDS: usize = 256;
/// Records copied per read-lock acquisition by body-filter scans.
pub(crate) const SCAN_CHUNK: usize = 2048;
/// Upper bound on scan threads.
const MAX_SCAN_THREADS: usize = 8;
/// Header names are collected from the most recent records only (bounded cost).
const HEADER_SCAN_RECORDS: usize = 5000;
const MAX_HOSTS: usize = 200;
const MAX_OTHER_SUGGESTIONS: usize = 100;

pub(crate) type Records = BTreeMap<ExchangeId, Box<Record>>;

/// A borrowed record as seen by the filter evaluator. Body text is decoded at most once per
/// view and only if the filter actually asks for it.
pub(crate) struct RecordView<'a> {
    rec: &'a Record,
    spill: &'a Spill,
    titles: &'a RuleTitles,
    request_text: OnceCell<Option<String>>,
    response_text: OnceCell<Option<String>>,
}

impl<'a> RecordView<'a> {
    pub(crate) fn new(rec: &'a Record, spill: &'a Spill, titles: &'a RuleTitles) -> Self {
        RecordView {
            rec,
            spill,
            titles,
            request_text: OnceCell::new(),
            response_text: OnceCell::new(),
        }
    }
}

/// Decoded body text for searching: content-encoding removed (raw bytes are used when that
/// fails), charset-decoded; `None` when the body is empty or binary.
pub(crate) fn body_text(
    body: &StoredBody,
    headers: Option<&Headers>,
    path: &str,
    spill: &Spill,
) -> Option<String> {
    if body.captured_len() == 0 {
        return None;
    }
    let headers = headers?;
    decode_text(body.load(spill).ok()?, headers, path)
}

fn decode_text(raw: bytes::Bytes, headers: &Headers, path: &str) -> Option<String> {
    let encodings = headers.content_encodings();
    let decoded = if encodings.is_empty() {
        raw
    } else {
        akev_content::decode::decode_body(&raw, &encodings, FILTER_MAX_DECODE).unwrap_or(raw)
    };
    let ct = headers.get("content-type");
    let kind = akev_content::detect::detect_kind(ct, &decoded[..decoded.len().min(4096)], path);
    if !akev_content::detect::is_textual(kind) && std::str::from_utf8(&decoded).is_err() {
        return None;
    }
    Some(akev_content::text::decode_text(&decoded, ct).0)
}

impl FilterTarget for RecordView<'_> {
    fn summary(&self) -> &ExchangeSummary {
        &self.rec.summary
    }

    fn request_headers(&self) -> &Headers {
        &self.rec.request_headers
    }

    fn response_headers(&self) -> Option<&Headers> {
        self.rec.response.as_ref().map(|r| &r.headers)
    }

    fn request_body_text(&self) -> Option<Cow<'_, str>> {
        self.request_text
            .get_or_init(|| {
                let path = self.rec.summary.path.split('?').next().unwrap_or("");
                body_text(
                    &self.rec.request_body,
                    Some(&self.rec.request_headers),
                    path,
                    self.spill,
                )
            })
            .as_deref()
            .map(Cow::Borrowed)
    }

    fn response_body_text(&self) -> Option<Cow<'_, str>> {
        self.response_text
            .get_or_init(|| {
                let path = self.rec.summary.path.split('?').next().unwrap_or("");
                let headers = self.rec.response.as_ref().map(|r| &r.headers);
                body_text(&self.rec.response_body, headers, path, self.spill)
            })
            .as_deref()
            .map(Cow::Borrowed)
    }

    fn client_ip(&self) -> Option<&str> {
        util::ip_of(&self.rec.client.remote_addr)
    }

    fn note(&self) -> Option<&str> {
        self.rec.note()
    }

    /// Title of the handling rule, from the titles registered via `Store::set_rule_titles`.
    fn rule_title(&self) -> Option<&str> {
        let rule = self.rec.summary.rule.as_ref()?;
        self.titles.get(rule).map(String::as_str)
    }
}

/// Rule id → title, used by `rule:` filter terms.
pub(crate) type RuleTitles = std::collections::HashMap<String, String>;

fn record_matches(filter: &Filter, rec: &Record, spill: &Spill, titles: &RuleTitles) -> bool {
    filter.matches(&RecordView::new(rec, spill, titles))
}

/// What the filter evaluator reads from a record, copied out under the store lock so body
/// text can be loaded (and decompressed) after the lock is released.
pub(crate) struct OwnedRecord {
    summary: ExchangeSummary,
    request_headers: Headers,
    response_headers: Option<Headers>,
    request_body: BodyRef,
    response_body: BodyRef,
    client_ip: Option<String>,
    note: Option<String>,
    rule_title: Option<String>,
}

impl OwnedRecord {
    pub(crate) fn of(rec: &Record, titles: &RuleTitles) -> Self {
        OwnedRecord {
            summary: rec.summary.clone(),
            request_headers: rec.request_headers.clone(),
            response_headers: rec.response.as_ref().map(|r| r.headers.clone()),
            request_body: rec.request_body.to_ref(),
            response_body: rec.response_body.to_ref(),
            client_ip: util::ip_of(&rec.client.remote_addr).map(str::to_string),
            note: rec.note().map(str::to_string),
            rule_title: rec
                .summary
                .rule
                .as_ref()
                .and_then(|r| titles.get(r))
                .cloned(),
        }
    }

    pub(crate) fn id(&self) -> ExchangeId {
        self.summary.id
    }
}

/// An [`OwnedRecord`] as seen by the filter evaluator.
struct OwnedView<'a> {
    rec: &'a OwnedRecord,
    spill: &'a Spill,
    request_text: OnceCell<Option<String>>,
    response_text: OnceCell<Option<String>>,
}

impl OwnedView<'_> {
    fn text(&self, body: &BodyRef, headers: Option<&Headers>) -> Option<String> {
        if body.captured_len() == 0 {
            return None;
        }
        let path = self.rec.summary.path.split('?').next().unwrap_or("");
        decode_text(body.load(self.spill).ok()?, headers?, path)
    }
}

impl FilterTarget for OwnedView<'_> {
    fn summary(&self) -> &ExchangeSummary {
        &self.rec.summary
    }

    fn request_headers(&self) -> &Headers {
        &self.rec.request_headers
    }

    fn response_headers(&self) -> Option<&Headers> {
        self.rec.response_headers.as_ref()
    }

    fn request_body_text(&self) -> Option<Cow<'_, str>> {
        self.request_text
            .get_or_init(|| self.text(&self.rec.request_body, Some(&self.rec.request_headers)))
            .as_deref()
            .map(Cow::Borrowed)
    }

    fn response_body_text(&self) -> Option<Cow<'_, str>> {
        self.response_text
            .get_or_init(|| self.text(&self.rec.response_body, self.rec.response_headers.as_ref()))
            .as_deref()
            .map(Cow::Borrowed)
    }

    fn client_ip(&self) -> Option<&str> {
        self.rec.client_ip.as_deref()
    }

    fn note(&self) -> Option<&str> {
        self.rec.note.as_deref()
    }

    fn rule_title(&self) -> Option<&str> {
        self.rec.rule_title.as_deref()
    }
}

/// Whether one copied record matches (no store lock needed).
pub(crate) fn owned_matches(filter: &Filter, rec: &OwnedRecord, spill: &Spill) -> bool {
    filter.matches(&OwnedView {
        rec,
        spill,
        request_text: OnceCell::new(),
        response_text: OnceCell::new(),
    })
}

/// Records matching `filter` in id order, optionally only the last `limit` of them. Meant for
/// filters without body terms, which run under the caller's read lock; body filters go
/// through [`OwnedRecord`] chunks and [`matching_owned`].
pub(crate) fn matching<'a>(
    records: &'a Records,
    spill: &'a Spill,
    titles: &RuleTitles,
    filter: &Filter,
    limit: Option<usize>,
) -> Vec<&'a Record> {
    if limit == Some(0) {
        return Vec::new();
    }
    let all = filter.is_match_all();
    let keep = |rec: &&Record| all || record_matches(filter, rec, spill, titles);
    match limit {
        Some(limit) => {
            let mut out: Vec<&Record> = records
                .values()
                .rev()
                .map(|r| &**r)
                .filter(keep)
                .take(limit)
                .collect();
            out.reverse();
            out
        }
        None => records.values().map(|r| &**r).filter(keep).collect(),
    }
}

/// Ids of the copied records matching `filter`, in input order. Large chunks are evaluated
/// on several threads (bodies may need disk reads and decompression); falls back to the
/// calling thread if threads can't be started.
pub(crate) fn matching_owned(
    records: &[OwnedRecord],
    spill: &Spill,
    filter: &Filter,
) -> Vec<ExchangeId> {
    let eval = |part: &[OwnedRecord]| -> Vec<ExchangeId> {
        part.iter()
            .filter(|r| owned_matches(filter, r, spill))
            .map(OwnedRecord::id)
            .collect()
    };
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .clamp(1, MAX_SCAN_THREADS);
    if threads == 1 || records.len() < PARALLEL_MIN_RECORDS {
        return eval(records);
    }
    let chunk = records.len().div_ceil(threads);
    std::thread::scope(|scope| {
        let mut pending = Vec::with_capacity(threads);
        for part in records.chunks(chunk) {
            match std::thread::Builder::new()
                .name("akev-store-scan".into())
                .spawn_scoped(scope, move || eval(part))
            {
                Ok(handle) => pending.push(Ok(handle)),
                // Couldn't start a thread: evaluate this chunk on the calling thread instead.
                Err(_) => pending.push(Err(part)),
            }
        }
        let mut out = Vec::new();
        for p in pending {
            match p {
                Ok(handle) => match handle.join() {
                    Ok(v) => out.extend(v),
                    Err(panic) => std::panic::resume_unwind(panic),
                },
                Err(part) => out.extend(eval(part)),
            }
        }
        out
    })
}

fn ranked<K: AsRef<str>>(counts: HashMap<K, u32>, cap: usize) -> Vec<String> {
    let mut v: Vec<(K, u32)> = counts.into_iter().collect();
    v.sort_unstable_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.as_ref().cmp(b.0.as_ref())));
    v.into_iter()
        .take(cap)
        .map(|(k, _)| k.as_ref().to_string())
        .collect()
}

/// Distinct values (most frequent first) for filter autocompletion.
pub(crate) fn suggest_context(records: &Records) -> SuggestContext {
    let mut hosts: HashMap<&str, u32> = HashMap::new();
    let mut methods: HashMap<&str, u32> = HashMap::new();
    let mut sources: HashMap<&str, u32> = HashMap::new();
    let mut content_types: HashMap<&str, u32> = HashMap::new();
    let mut models: HashMap<&str, u32> = HashMap::new();
    let mut providers: HashMap<&str, u32> = HashMap::new();
    for rec in records.values() {
        let s = &rec.summary;
        if let Some(llm) = &s.llm {
            *providers.entry(&llm.provider).or_default() += 1;
            if let Some(m) = llm.model.as_deref().filter(|m| !m.is_empty()) {
                *models.entry(m).or_default() += 1;
            }
        }
        if !s.host.is_empty() {
            *hosts.entry(&s.host).or_default() += 1;
        }
        if !s.method.is_empty() {
            *methods.entry(&s.method).or_default() += 1;
        }
        if let Some(src) = &s.source {
            *sources.entry(src).or_default() += 1;
        }
        if let Some(ct) = s.content_type.as_deref().filter(|c| !c.is_empty()) {
            *content_types.entry(ct).or_default() += 1;
        }
    }
    let mut header_names: HashMap<String, u32> = HashMap::new();
    let mut buf = [0u8; 64];
    for rec in records.values().rev().take(HEADER_SCAN_RECORDS) {
        let response = rec.response.as_ref().map(|r| r.headers.iter());
        for (name, _) in rec
            .request_headers
            .iter()
            .chain(response.into_iter().flatten())
        {
            if name.is_empty() || name.starts_with(':') {
                continue;
            }
            let lower = util::lower_name(name, &mut buf);
            match header_names.get_mut(lower.as_ref()) {
                Some(n) => *n += 1,
                None => {
                    header_names.insert(lower.into_owned(), 1);
                }
            }
        }
    }
    SuggestContext {
        hosts: ranked(hosts, MAX_HOSTS),
        methods: ranked(methods, MAX_OTHER_SUGGESTIONS),
        sources: ranked(sources, MAX_OTHER_SUGGESTIONS),
        content_types: ranked(content_types, MAX_OTHER_SUGGESTIONS),
        header_names: ranked(header_names, MAX_OTHER_SUGGESTIONS),
        models: ranked(models, MAX_OTHER_SUGGESTIONS),
        providers: ranked(providers, MAX_OTHER_SUGGESTIONS),
    }
}
