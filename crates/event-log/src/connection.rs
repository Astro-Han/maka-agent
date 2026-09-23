/*
 * Licensed to the Apache Software Foundation (ASF) under one
 * or more contributor license agreements.  See the NOTICE file
 * distributed with this work for additional information
 * regarding copyright ownership.  The ASF licenses this file
 * to you under the Apache License, Version 2.0 (the
 * "License"); you may not use this file except in compliance
 * with the License.  You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing,
 * software distributed under the License is distributed on an
 * "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
 * KIND, either express or implied.  See the License for the
 * specific language governing permissions and limitations
 * under the License.
 */

use std::{future::Future, panic::AssertUnwindSafe, pin::Pin, sync::Arc};

mod authority;
pub use authority::ConnectionAuthority;

use futures_util::{FutureExt, future::Shared};
use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};
use tokio::sync::{Semaphore, mpsc, oneshot};

use crate::StoreError;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

type Job = Box<
    dyn for<'c> FnOnce(&'c mut SqliteConnection, Result<(), StoreError>) -> BoxFuture<'c, ()>
        + Send,
>;
type ReadJob =
    Box<dyn FnOnce(Result<SqliteConnection, StoreError>) -> BoxFuture<'static, ()> + Send>;
enum Command {
    Run(Job),
    Read(ReadJob),
    Shutdown,
}

/// Ingress is bounded; accepted work and the database lease belong to a dedicated
/// thread, independent of the caller's executor. Dropping ingress drains work.
pub struct OwnedConnection {
    jobs: mpsc::Sender<Command>,
    closed: Shared<BoxFuture<'static, Result<(), String>>>,
    readers: Arc<Semaphore>,
}

impl OwnedConnection {
    pub async fn open(
        options: SqliteConnectOptions,
        authority: ConnectionAuthority,
        initialize: for<'c> fn(&'c mut SqliteConnection) -> BoxFuture<'c, Result<(), StoreError>>,
    ) -> Result<Self, StoreError> {
        let (jobs, mut receiver) = mpsc::channel::<Command>(32);
        let authority = Arc::new(authority);
        let (ready, startup) = oneshot::channel();
        let (completion, closed) = oneshot::channel();
        std::thread::Builder::new()
            .name("runtime-store-owner".into())
            .spawn(move || {
                let result = (|| {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()?;
                    let validate = || authority.validate();
                    validate()?;
                    let mut connection =
                        runtime.block_on(SqliteConnection::connect_with(&options))?;
                    let read_options = options.clone().create_if_missing(false).read_only(true);
                    let mut readers = tokio::task::JoinSet::new();
                    let initialized = std::panic::catch_unwind(AssertUnwindSafe(|| {
                        runtime.block_on(async {
                            initialize(&mut connection).await?;
                            validate()
                        })
                    }))
                    .unwrap_or(Err(StoreError::OperationUnknown));
                    let success = initialized.is_ok();
                    // If open's waiter disappeared, close without accepting work.
                    if ready.send(initialized).is_ok() && success {
                        while let Some(command) = runtime.block_on(receiver.recv()) {
                            while readers.try_join_next().is_some() {}
                            let job = match command {
                                Command::Run(job) => job,
                                Command::Read(job) => {
                                    let options = read_options.clone();
                                    let valid = validate();
                                    let authority = authority.clone();
                                    readers.spawn_on(
                                        async move {
                                            let connection = match valid {
                                                Ok(()) => SqliteConnection::connect_with(&options)
                                                    .await
                                                    .map_err(StoreError::from),
                                                Err(error) => Err(error),
                                            };
                                            let connection = match connection {
                                                Ok(connection) => match authority.validate() {
                                                    Ok(()) => Ok(connection),
                                                    Err(error) => {
                                                        let _ = connection.close().await;
                                                        Err(error)
                                                    }
                                                },
                                                Err(error) => Err(error),
                                            };
                                            job(connection).await;
                                        },
                                        runtime.handle(),
                                    );
                                    continue;
                                }
                                Command::Shutdown => {
                                    // Reject new sends, then drain every accepted job.
                                    receiver.close();
                                    continue;
                                }
                            };
                            let executed = std::panic::catch_unwind(AssertUnwindSafe(|| {
                                runtime.block_on(job(&mut connection, validate()))
                            }));
                            if executed.is_err() {
                                // An unwound connection cannot accept further jobs.
                                break;
                            }
                        }
                    }
                    // Readers share this owner's authority, not the caller's lifetime.
                    // Close them before releasing the writer lease or acknowledging shutdown.
                    runtime.block_on(async { while readers.join_next().await.is_some() {} });
                    // Await real close before releasing authority, even after failure.
                    runtime
                        .block_on(connection.close())
                        .map_err(StoreError::from)
                })();
                drop(receiver);
                drop(authority);
                let _ = completion.send(result);
            })?;
        match startup.await {
            Ok(Ok(())) => Ok(Self {
                jobs,
                closed: async move {
                    closed
                        .await
                        .map_err(|_| {
                            "connection owner exited without close acknowledgement".to_owned()
                        })?
                        .map_err(|error| error.to_string())
                }
                .boxed()
                .shared(),
                readers: Arc::new(Semaphore::new(2)),
            }),
            Ok(Err(error)) => {
                drop(jobs);
                let _ = closed.await;
                Err(error)
            }
            Err(_) => {
                drop(jobs);
                closed.await.map_err(|_| StoreError::OperationUnknown)??;
                Err(StoreError::Initialization)
            }
        }
    }

    pub async fn run<T: Send + 'static>(
        &self,
        operation: impl for<'c> FnOnce(&'c mut SqliteConnection) -> BoxFuture<'c, Result<T, StoreError>>
        + Send
        + 'static,
    ) -> Result<T, StoreError> {
        let (reply, result) = oneshot::channel();
        self.jobs
            .send(Command::Run(Box::new(move |connection, valid| {
                Box::pin(async move {
                    let result = match valid {
                        Ok(()) => operation(connection).await,
                        Err(error) => Err(error),
                    };
                    let _ = reply.send(result);
                })
            })))
            .await
            .map_err(|_| StoreError::ConnectionClosed)?;
        result.await.map_err(|_| StoreError::OperationUnknown)?
    }

    pub async fn shutdown(&self) -> Result<(), StoreError> {
        self.readers.close();
        // Closed ingress is also normal for repeated or concurrent shutdown.
        let _ = self.jobs.send(Command::Shutdown).await;
        self.closed.clone().await.map_err(StoreError::CloseFailed)
    }

    /// Bounded independent readers keep large snapshots off the serialized writer.
    /// Accepted reads drain on shutdown even if their caller or executor disappears.
    pub(crate) async fn read<T: Send + 'static>(
        &self,
        operation: impl for<'c> FnOnce(&'c mut SqliteConnection) -> BoxFuture<'c, Result<T, StoreError>>
        + Send
        + 'static,
    ) -> Result<T, StoreError> {
        let permit = self
            .readers
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| StoreError::ConnectionClosed)?;
        let (reply, result) = oneshot::channel();
        self.jobs
            .send(Command::Read(Box::new(move |connection| {
                Box::pin(async move {
                    let _permit = permit;
                    let result = match connection {
                        Ok(mut connection) => {
                            let result =
                                AssertUnwindSafe(async { operation(&mut connection).await })
                                    .catch_unwind()
                                    .await
                                    .unwrap_or(Err(StoreError::OperationUnknown));
                            let closed = connection.close().await.map_err(StoreError::from);
                            match (result, closed) {
                                (Ok(value), Ok(())) => Ok(value),
                                (Err(error), _) | (_, Err(error)) => Err(error),
                            }
                        }
                        Err(error) => Err(error),
                    };
                    let _ = reply.send(result);
                })
            })))
            .await
            .map_err(|_| StoreError::ConnectionClosed)?;
        result.await.map_err(|_| StoreError::OperationUnknown)?
    }

    pub async fn close(self) -> Result<(), StoreError> {
        self.shutdown().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs::{File, OpenOptions},
        sync::{Arc, mpsc as blocking},
        time::Duration,
    };

    #[tokio::test]
    async fn read_snapshot_does_not_block_writes_and_shutdown_drains_abandoned_readers() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("snapshot.sqlite");
        let lock = directory.path().join("writer.lock");
        let lease = Arc::new(crate::root::FileLease::acquire(&lock).unwrap());
        let contender = File::open(&lock).unwrap();
        let owner = Arc::new(OwnedConnection::open(
            SqliteConnectOptions::new().filename(&path).create_if_missing(true),
            ConnectionAuthority::Writer { lease, root: None },
            |connection| Box::pin(async move {
                sqlx::raw_sql("PRAGMA journal_mode=WAL; CREATE TABLE facts(value INTEGER); INSERT INTO facts VALUES(1);")
                    .execute(connection).await?;
                Ok(())
            }),
        ).await.unwrap());
        let (ready, opened) = oneshot::channel();
        let (release, gate) = oneshot::channel();
        let (finished, observed) = oneshot::channel();
        let reader = owner.clone();
        let caller = tokio::spawn(async move {
            reader
                .read(move |connection| {
                    Box::pin(async move {
                        let mut tx = connection.begin().await?;
                        let first: i64 = sqlx::query_scalar("SELECT SUM(value) FROM facts")
                            .fetch_one(&mut *tx)
                            .await?;
                        ready.send(()).unwrap();
                        gate.await.unwrap();
                        let second: i64 = sqlx::query_scalar("SELECT SUM(value) FROM facts")
                            .fetch_one(&mut *tx)
                            .await?;
                        assert!(
                            sqlx::query("INSERT INTO facts VALUES(99)")
                                .execute(&mut *tx)
                                .await
                                .is_err(),
                            "reader must not promote to a writer"
                        );
                        tx.rollback().await?;
                        finished.send((first, second)).unwrap();
                        Ok(())
                    })
                })
                .await
        });
        tokio::time::timeout(Duration::from_secs(5), opened)
            .await
            .unwrap()
            .unwrap();
        tokio::time::timeout(
            Duration::from_secs(5),
            owner.run(|connection| {
                Box::pin(async move {
                    sqlx::query("INSERT INTO facts VALUES(2)")
                        .execute(connection)
                        .await?;
                    Ok(())
                })
            }),
        )
        .await
        .unwrap()
        .unwrap();
        caller.abort();
        let _ = caller.await;
        let closing = owner.clone();
        let shutdown = tokio::spawn(async move { closing.shutdown().await });
        assert!(
            tokio::time::timeout(Duration::from_millis(50), owner.closed.clone())
                .await
                .is_err()
        );
        assert!(matches!(
            contender.try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        ));
        release.send(()).unwrap();
        assert_eq!(observed.await.unwrap(), (1, 1));
        tokio::time::timeout(Duration::from_secs(5), shutdown)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        contender.try_lock().unwrap();
        assert!(matches!(
            owner.read(|_| Box::pin(async { Ok(()) })).await,
            Err(StoreError::ConnectionClosed)
        ));
    }

    #[test]
    fn dropped_caller_runtime_keeps_authority_until_real_commit_and_close() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("owner.sqlite");
        let lock = directory.path().join("writer.lock");
        let lease = Arc::new(crate::root::FileLease::acquire(&lock).unwrap());
        let contender = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lock)
            .unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let options = SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true);
        let owner = runtime
            .block_on(OwnedConnection::open(
                options,
                ConnectionAuthority::Writer { lease, root: None },
                |connection| {
                    Box::pin(async move {
                        sqlx::query("CREATE TABLE committed(value INTEGER)")
                            .execute(connection)
                            .await?;
                        Ok(())
                    })
                },
            ))
            .unwrap();
        let (entered, reached) = blocking::channel();
        let (release, gate) = blocking::channel();
        let close_ack = owner.closed.clone();
        runtime.spawn(async move {
            owner
                .run(move |connection| {
                    Box::pin(async move {
                        connection.lock_handle().await?.set_commit_hook(move || {
                            entered.send(()).unwrap();
                            gate.recv_timeout(Duration::from_secs(10)).unwrap();
                            true
                        });
                        sqlx::query("INSERT INTO committed VALUES (7)")
                            .execute(connection)
                            .await?;
                        Ok(())
                    })
                })
                .await
                .unwrap();
        });
        runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(5), async {
                while reached.try_recv().is_err() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
        });
        drop(runtime);
        assert!(matches!(
            contender.try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        ));
        release.send(()).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            close_ack.await.unwrap();
            contender.try_lock().unwrap();
            let mut connection =
                SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(&path))
                    .await
                    .unwrap();
            assert_eq!(
                sqlx::query_scalar::<_, i64>("SELECT value FROM committed")
                    .fetch_one(&mut connection)
                    .await
                    .unwrap(),
                7
            );
            connection.close().await.unwrap();
        });
    }

    #[tokio::test]
    async fn shutdown_is_idempotent_while_handles_remain_alive() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("writer.lock");
        let lease = Arc::new(crate::root::FileLease::acquire(&path).unwrap());
        let contender = File::open(&path).unwrap();
        let owner = Arc::new(
            OwnedConnection::open(
                SqliteConnectOptions::new().in_memory(true),
                ConnectionAuthority::Writer { lease, root: None },
                |_| Box::pin(async { Ok(()) }),
            )
            .await
            .unwrap(),
        );
        let retained = owner.clone();
        // Replacing the pathname must prevent the old worker from accepting new work.
        std::fs::rename(&path, directory.path().join("displaced.lock")).unwrap();
        let replacement = crate::root::FileLease::acquire(&path).unwrap();
        assert!(matches!(
            retained
                .run::<()>(|_| Box::pin(async { panic!("replaced lease must reject work") }))
                .await,
            Err(StoreError::Io(_))
        ));
        drop(replacement);
        let (first, second) = tokio::join!(owner.shutdown(), retained.shutdown());
        first.unwrap();
        second.unwrap();
        retained.shutdown().await.unwrap();
        contender.try_lock().unwrap();
        assert!(matches!(
            retained.run(|_| Box::pin(async { Ok(()) })).await,
            Err(StoreError::ConnectionClosed)
        ));
    }
}
