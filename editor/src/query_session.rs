use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex as StdMutex},
};

use sqlab_drivers_core::{
    DataSource, DataSourceConfig, DataSourceError, QueryExecutionOptions, QueryResult,
};
use tokio::sync::Mutex;

use crate::drivers::create_configured_data_source;

#[derive(Clone, Default)]
pub struct QuerySessionStore {
    sessions: Arc<Mutex<HashMap<PathBuf, HashMap<String, QuerySession>>>>,
    open_sessions: Arc<StdMutex<HashMap<PathBuf, HashMap<String, ()>>>>,
    closing_sessions: Arc<StdMutex<HashMap<PathBuf, HashSet<String>>>>,
}

struct QuerySession {
    source: Box<dyn DataSource>,
}

impl QuerySessionStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_open(&self, path: &Path, name: &str) -> bool {
        let is_open = self
            .open_sessions
            .lock()
            .map(|sessions| {
                sessions
                    .get(path)
                    .is_some_and(|conns| conns.contains_key(name))
            })
            .unwrap_or(false);
        let is_closing = self
            .closing_sessions
            .lock()
            .map(|sessions| sessions.get(path).is_some_and(|conns| conns.contains(name)))
            .unwrap_or(false);
        is_open && !is_closing
    }

    pub fn mark_closing(&self, path: PathBuf, name: String) {
        if let Ok(mut sessions) = self.closing_sessions.lock() {
            sessions.entry(path).or_default().insert(name);
        }
    }

    pub fn is_connection_open(&self, name: &str) -> bool {
        self.open_sessions
            .lock()
            .map(|sessions| sessions.values().any(|conns| conns.contains_key(name)))
            .unwrap_or(false)
    }

    pub async fn execute_query(
        &self,
        path: PathBuf,
        config: DataSourceConfig,
        options: QueryExecutionOptions,
        query: String,
    ) -> Result<QueryResult, DataSourceError> {
        let mut sessions = self.sessions.lock().await;

        self.ensure_connected(&path, &config, &mut sessions).await?;

        let result = {
            let session = sessions
                .get(&path)
                .and_then(|conns| conns.get(&config.name))
                .ok_or(DataSourceError::NotConnected)?;
            session
                .source
                .execute_query_with_options(&query, &options)
                .await
        };

        if matches!(
            result,
            Err(DataSourceError::ConnectionFailed(_) | DataSourceError::NotConnected)
        ) {
            let _ = self
                .disconnect_locked(&path, &config.name, &mut sessions)
                .await;
            self.ensure_connected(&path, &config, &mut sessions).await?;

            let session = sessions
                .get(&path)
                .and_then(|conns| conns.get(&config.name))
                .ok_or(DataSourceError::NotConnected)?;
            return session
                .source
                .execute_query_with_options(&query, &options)
                .await;
        }

        result
    }

    pub async fn close_path(&self, path: PathBuf) -> Result<(), DataSourceError> {
        let mut sessions = self.sessions.lock().await;
        let names: Vec<String> = sessions
            .get(&path)
            .map(|conns| conns.keys().cloned().collect())
            .unwrap_or_default();
        for name in names {
            self.disconnect_locked(&path, &name, &mut sessions).await?;
        }
        Ok(())
    }

    pub async fn close_path_connection(
        &self,
        path: PathBuf,
        name: String,
    ) -> Result<(), DataSourceError> {
        let mut sessions = self.sessions.lock().await;
        self.disconnect_locked(&path, &name, &mut sessions).await
    }

    pub async fn close_all(&self) -> Result<(), DataSourceError> {
        let mut sessions = self.sessions.lock().await;
        let entries: Vec<(PathBuf, Vec<String>)> = sessions
            .iter()
            .map(|(path, conns)| (path.clone(), conns.keys().cloned().collect()))
            .collect();
        for (path, names) in entries {
            for name in names {
                self.disconnect_locked(&path, &name, &mut sessions).await?;
            }
        }
        Ok(())
    }

    pub async fn close_connection_name(&self, name: String) -> Result<(), DataSourceError> {
        let mut sessions = self.sessions.lock().await;
        let paths: Vec<PathBuf> = sessions
            .iter()
            .filter(|(_, conns)| conns.contains_key(&name))
            .map(|(path, _)| path.clone())
            .collect();
        for path in paths {
            self.disconnect_locked(&path, &name, &mut sessions).await?;
        }
        Ok(())
    }

    async fn ensure_connected(
        &self,
        path: &Path,
        config: &DataSourceConfig,
        sessions: &mut HashMap<PathBuf, HashMap<String, QuerySession>>,
    ) -> Result<(), DataSourceError> {
        if sessions
            .get(path)
            .is_some_and(|conns| conns.contains_key(&config.name))
        {
            return Ok(());
        }
        let mut source = create_configured_data_source(config)?;
        source.connect().await?;
        let name = config.name.clone();
        sessions
            .entry(path.to_path_buf())
            .or_default()
            .insert(name.clone(), QuerySession { source });
        self.mark_open(path.to_path_buf(), name);
        self.unmark_closing(path, &config.name);
        Ok(())
    }

    async fn disconnect_locked(
        &self,
        path: &Path,
        name: &str,
        sessions: &mut HashMap<PathBuf, HashMap<String, QuerySession>>,
    ) -> Result<(), DataSourceError> {
        let Some(mut session) = sessions.get_mut(path).and_then(|conns| conns.remove(name)) else {
            self.mark_closed(path, name);
            return Ok(());
        };
        if sessions.get(path).is_some_and(|conns| conns.is_empty()) {
            sessions.remove(path);
        }
        self.mark_closed(path, name);
        session.source.disconnect().await
    }

    fn mark_open(&self, path: PathBuf, name: String) {
        if let Ok(mut sessions) = self.open_sessions.lock() {
            sessions.entry(path).or_default().insert(name, ());
        }
    }

    fn mark_closed(&self, path: &Path, name: &str) {
        if let Ok(mut sessions) = self.open_sessions.lock()
            && let Some(conns) = sessions.get_mut(path)
        {
            conns.remove(name);
            if conns.is_empty() {
                sessions.remove(path);
            }
        }
        self.unmark_closing(path, name);
    }

    fn unmark_closing(&self, path: &Path, name: &str) {
        if let Ok(mut sessions) = self.closing_sessions.lock()
            && let Some(conns) = sessions.get_mut(path)
        {
            conns.remove(name);
            if conns.is_empty() {
                sessions.remove(path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlab_drivers_core::Database;
    use tokio::runtime::Runtime;

    fn sqlite_config(name: &str) -> DataSourceConfig {
        DataSourceConfig {
            name: name.into(),
            db_type: Database::SQLite,
            database: String::new(),
            ..Default::default()
        }
    }

    fn run_query(
        store: &QuerySessionStore,
        path: &Path,
        config_name: &str,
        query: &str,
    ) -> Result<QueryResult, DataSourceError> {
        Runtime::new().unwrap().block_on(store.execute_query(
            path.to_path_buf(),
            sqlite_config(config_name),
            QueryExecutionOptions::default(),
            query.to_string(),
        ))
    }

    #[test]
    fn reuses_session_for_same_file_and_connection() {
        let store = QuerySessionStore::new();
        let path = PathBuf::from("/tmp/sqlab-session-test.sql");

        run_query(&store, &path, "memory", "create table items (id integer);").unwrap();
        run_query(&store, &path, "memory", "insert into items values (1);").unwrap();
        let result = run_query(&store, &path, "memory", "select id from items;").unwrap();

        assert!(store.is_open(&path, "memory"));
        assert_eq!(result.rows, vec![vec!["1".to_string()]]);
    }

    #[test]
    fn preserves_sessions_across_connection_switches() {
        let store = QuerySessionStore::new();
        let path = PathBuf::from("/tmp/sqlab-session-switch-test.sql");

        run_query(&store, &path, "conn-1", "create table t1 (id integer);").unwrap();
        run_query(&store, &path, "conn-1", "insert into t1 values (42);").unwrap();

        run_query(&store, &path, "conn-2", "create table t2 (val text);").unwrap();
        run_query(&store, &path, "conn-2", "insert into t2 values ('hello');").unwrap();

        assert!(store.is_open(&path, "conn-1"));
        assert!(store.is_open(&path, "conn-2"));

        let result = run_query(&store, &path, "conn-1", "select id from t1;").unwrap();
        assert_eq!(result.rows, vec![vec!["42".to_string()]]);

        let result = run_query(&store, &path, "conn-2", "select val from t2;").unwrap();
        assert_eq!(result.rows, vec![vec!["hello".to_string()]]);
    }

    #[test]
    fn close_path_connection_only_affects_that_connection() {
        let store = QuerySessionStore::new();
        let path = PathBuf::from("/tmp/sqlab-session-close-conn-test.sql");

        run_query(&store, &path, "conn-1", "create table t1 (id integer);").unwrap();
        run_query(&store, &path, "conn-2", "create table t2 (id integer);").unwrap();

        Runtime::new()
            .unwrap()
            .block_on(store.close_path_connection(path.clone(), "conn-1".into()))
            .unwrap();

        assert!(!store.is_open(&path, "conn-1"));
        assert!(store.is_open(&path, "conn-2"));
    }

    #[test]
    fn close_connection_name_closes_that_connection_across_paths() {
        let store = QuerySessionStore::new();
        let path_1 = PathBuf::from("/tmp/sqlab-session-close-name-1.sql");
        let path_2 = PathBuf::from("/tmp/sqlab-session-close-name-2.sql");

        run_query(&store, &path_1, "shared", "create table t1 (id integer);").unwrap();
        run_query(&store, &path_2, "shared", "create table t2 (id integer);").unwrap();
        run_query(&store, &path_2, "other", "create table t3 (id integer);").unwrap();

        Runtime::new()
            .unwrap()
            .block_on(store.close_connection_name("shared".into()))
            .unwrap();

        assert!(!store.is_open(&path_1, "shared"));
        assert!(!store.is_open(&path_2, "shared"));
        assert!(store.is_open(&path_2, "other"));
    }

    #[test]
    fn reconnects_automatically_when_connection_drops() {
        let store = QuerySessionStore::new();
        let path = PathBuf::from("/tmp/sqlab-session-reconnect-test.sql");
        let _ = std::fs::remove_file(&path);
        let config = DataSourceConfig {
            name: "file-conn".into(),
            db_type: Database::SQLite,
            database: path.to_string_lossy().into_owned(),
            ..Default::default()
        };

        let rt = Runtime::new().unwrap();

        rt.block_on(store.execute_query(
            path.clone(),
            config.clone(),
            QueryExecutionOptions::default(),
            "create table t1 (id integer);".into(),
        ))
        .unwrap();

        rt.block_on(store.execute_query(
            path.clone(),
            config.clone(),
            QueryExecutionOptions::default(),
            "insert into t1 values (99);".into(),
        ))
        .unwrap();

        rt.block_on(async {
            let mut sessions = store.sessions.lock().await;
            let session = sessions
                .get_mut(&path)
                .and_then(|conns| conns.get_mut("file-conn"))
                .unwrap();
            session.source.disconnect().await.unwrap();
        });

        let result = rt.block_on(store.execute_query(
            path.clone(),
            config,
            QueryExecutionOptions::default(),
            "select id from t1;".into(),
        ));

        assert!(result.is_ok());
        assert_eq!(result.unwrap().rows, vec![vec!["99".to_string()]]);
        assert!(store.is_open(&path, "file-conn"));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn close_path_drops_all_sessions() {
        let store = QuerySessionStore::new();
        let path = PathBuf::from("/tmp/sqlab-session-close-test.sql");

        run_query(&store, &path, "conn-1", "create table t1 (id integer);").unwrap();
        run_query(&store, &path, "conn-2", "create table t2 (id integer);").unwrap();
        Runtime::new()
            .unwrap()
            .block_on(store.close_path(path.clone()))
            .unwrap();

        assert!(!store.is_open(&path, "conn-1"));
        assert!(!store.is_open(&path, "conn-2"));
        assert!(run_query(&store, &path, "conn-1", "select id from t1;").is_err());
    }
}
