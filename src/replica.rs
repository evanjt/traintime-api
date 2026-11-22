//! The Worker's one way into D1. Reads go through a session so the nearest
//! read replica can answer them instead of the primary in western Europe.
//! The Worker never writes: the seed runs through wrangler against the primary.

use std::sync::Arc;

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use worker::d1::{D1Database, D1PreparedStatement};
use worker::worker_sys::types::D1PreparedStatement as D1PreparedStatementSys;

/// Any replica may answer the first read, as stations change only on a reseed.
pub const FIRST_READ: &str = "first-unconstrained";

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(extends = js_sys::Object)]
    type DatabaseJs;

    #[wasm_bindgen(method, catch, js_name = withSession)]
    fn with_session(this: &DatabaseJs, constraint: &str) -> Result<SessionJs, JsValue>;

    #[wasm_bindgen(extends = js_sys::Object)]
    type SessionJs;

    #[wasm_bindgen(method, catch)]
    fn prepare(this: &SessionJs, query: &str) -> Result<D1PreparedStatementSys, JsValue>;
}

pub struct ReadSession(SessionJs);

// The Worker runs one request per isolate thread, as worker's own D1Database assumes.
unsafe impl Send for ReadSession {}
unsafe impl Sync for ReadSession {}

impl ReadSession {
    /// Opens the request's session. D1 without replication enabled serves it
    /// from the primary, so this is safe before the owner turns replication on.
    pub fn open(db: D1Database) -> worker::Result<Arc<Self>> {
        let js: &JsValue = db.as_ref();
        let session = js
            .unchecked_ref::<DatabaseJs>()
            .with_session(FIRST_READ)
            .map_err(worker::Error::from)?;
        Ok(Arc::new(Self(session)))
    }

    pub fn prepare(&self, query: &str) -> worker::Result<D1PreparedStatement> {
        self.0
            .prepare(query)
            .map(D1PreparedStatement::from)
            .map_err(worker::Error::from)
    }
}
