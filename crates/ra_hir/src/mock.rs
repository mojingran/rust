use std::{sync::Arc, panic};

use parking_lot::Mutex;
use ra_db::{
    FilePosition, FileId, CrateGraph, SourceRoot, SourceRootId, SourceDatabase, salsa,
};
use relative_path::RelativePathBuf;
use test_utils::{parse_fixture, CURSOR_MARKER, extract_offset};
use rustc_hash::FxHashMap;

use crate::{db, HirInterner};

pub const WORKSPACE: SourceRootId = SourceRootId(0);

#[salsa::database(
    ra_db::SourceDatabaseStorage,
    db::HirDatabaseStorage,
    db::PersistentHirDatabaseStorage
)]
#[derive(Debug)]
pub struct MockDatabase {
    events: Mutex<Option<Vec<salsa::Event<MockDatabase>>>>,
    runtime: salsa::Runtime<MockDatabase>,
    interner: Arc<HirInterner>,
    files: FxHashMap<String, FileId>,
}

impl panic::RefUnwindSafe for MockDatabase {}

impl MockDatabase {
    pub fn with_files(fixture: &str) -> MockDatabase {
        let (db, position) = MockDatabase::from_fixture(fixture);
        assert!(position.is_none());
        db
    }

    pub fn with_single_file(text: &str) -> (MockDatabase, SourceRoot, FileId) {
        let mut db = MockDatabase::default();
        let mut source_root = SourceRoot::default();
        let file_id = db.add_file(WORKSPACE, "/", &mut source_root, "/main.rs", text);
        db.set_source_root(WORKSPACE, Arc::new(source_root.clone()));
        (db, source_root, file_id)
    }

    pub fn with_position(fixture: &str) -> (MockDatabase, FilePosition) {
        let (db, position) = MockDatabase::from_fixture(fixture);
        let position = position.expect("expected a marker ( <|> )");
        (db, position)
    }

    pub fn file_id_of(&self, path: &str) -> FileId {
        match self.files.get(path) {
            Some(it) => *it,
            None => panic!("unknown file: {:?}\nexisting files:\n{:#?}", path, self.files),
        }
    }

    fn from_fixture(fixture: &str) -> (MockDatabase, Option<FilePosition>) {
        let mut db = MockDatabase::default();

        let pos = db.add_fixture(fixture);

        (db, pos)
    }

    fn add_fixture(&mut self, fixture: &str) -> Option<FilePosition> {
        let mut position = None;
        let mut source_root = SourceRoot::default();
        let mut source_root_id = WORKSPACE;
        let mut source_root_prefix = "/".to_string();
        for entry in parse_fixture(fixture) {
            if entry.meta.starts_with("root") {
                self.set_source_root(source_root_id, Arc::new(source_root));
                source_root = SourceRoot::default();

                source_root_id = SourceRootId(source_root_id.0 + 1);
                source_root_prefix = entry.meta["root".len()..].trim().to_string();
                continue;
            }
            if entry.text.contains(CURSOR_MARKER) {
                assert!(position.is_none(), "only one marker (<|>) per fixture is allowed");
                position = Some(self.add_file_with_position(
                    source_root_id,
                    &source_root_prefix,
                    &mut source_root,
                    &entry.meta,
                    &entry.text,
                ));
            } else {
                self.add_file(
                    source_root_id,
                    &source_root_prefix,
                    &mut source_root,
                    &entry.meta,
                    &entry.text,
                );
            }
        }
        self.set_source_root(source_root_id, Arc::new(source_root));
        position
    }

    fn add_file(
        &mut self,
        source_root_id: SourceRootId,
        source_root_prefix: &str,
        source_root: &mut SourceRoot,
        path: &str,
        text: &str,
    ) -> FileId {
        assert!(source_root_prefix.starts_with('/'));
        assert!(source_root_prefix.ends_with('/'));
        assert!(path.starts_with(source_root_prefix));
        let rel_path = RelativePathBuf::from_path(&path[source_root_prefix.len()..]).unwrap();

        let is_crate_root = rel_path == "lib.rs" || rel_path == "/main.rs";

        let file_id = FileId(self.files.len() as u32);
        let prev = self.files.insert(path.to_string(), file_id);
        assert!(prev.is_none(), "duplicate files in the text fixture");
        let text = Arc::new(text.to_string());
        self.set_file_text(file_id, text);
        self.set_file_relative_path(file_id, rel_path.clone());
        self.set_file_source_root(file_id, source_root_id);
        source_root.files.insert(rel_path, file_id);

        if is_crate_root {
            let mut crate_graph = CrateGraph::default();
            crate_graph.add_crate_root(file_id);
            self.set_crate_graph(Arc::new(crate_graph));
        }
        file_id
    }

    fn add_file_with_position(
        &mut self,
        source_root_id: SourceRootId,
        source_root_prefix: &str,
        source_root: &mut SourceRoot,
        path: &str,
        text: &str,
    ) -> FilePosition {
        let (offset, text) = extract_offset(text);
        let file_id = self.add_file(source_root_id, source_root_prefix, source_root, path, &text);
        FilePosition { file_id, offset }
    }
}

impl salsa::Database for MockDatabase {
    fn salsa_runtime(&self) -> &salsa::Runtime<MockDatabase> {
        &self.runtime
    }

    fn salsa_event(&self, event: impl Fn() -> salsa::Event<MockDatabase>) {
        let mut events = self.events.lock();
        if let Some(events) = &mut *events {
            events.push(event());
        }
    }
}

impl Default for MockDatabase {
    fn default() -> MockDatabase {
        let mut db = MockDatabase {
            events: Default::default(),
            runtime: salsa::Runtime::default(),
            interner: Default::default(),
            files: FxHashMap::default(),
        };
        db.set_crate_graph(Default::default());
        db
    }
}

impl salsa::ParallelDatabase for MockDatabase {
    fn snapshot(&self) -> salsa::Snapshot<MockDatabase> {
        salsa::Snapshot::new(MockDatabase {
            events: Default::default(),
            runtime: self.runtime.snapshot(self),
            interner: Arc::clone(&self.interner),
            // only the root database can be used to get file_id by path.
            files: FxHashMap::default(),
        })
    }
}

impl AsRef<HirInterner> for MockDatabase {
    fn as_ref(&self) -> &HirInterner {
        &self.interner
    }
}

impl MockDatabase {
    pub fn log(&self, f: impl FnOnce()) -> Vec<salsa::Event<MockDatabase>> {
        *self.events.lock() = Some(Vec::new());
        f();
        self.events.lock().take().unwrap()
    }

    pub fn log_executed(&self, f: impl FnOnce()) -> Vec<String> {
        let events = self.log(f);
        events
            .into_iter()
            .filter_map(|e| match e.kind {
                // This pretty horrible, but `Debug` is the only way to inspect
                // QueryDescriptor at the moment.
                salsa::EventKind::WillExecute { database_key } => {
                    Some(format!("{:?}", database_key))
                }
                _ => None,
            })
            .collect()
    }
}
