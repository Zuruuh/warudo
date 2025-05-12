use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

use derive_builder::Builder;
use watchexec_events::{Event, Tag, filekind::FileEventKind};

use crate::Arguments;

#[derive(Debug, Clone, Eq, PartialEq, PartialOrd, Ord)]
enum OperationType {
    Create,
    Update,
    Delete,
}

#[derive(Debug, Builder, Eq, PartialEq, PartialOrd, Ord)]
struct Operation {
    path: PathBuf,
    kind: OperationType,
    filetype: Option<FileType>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
struct FileType(watchexec_events::FileType);

fn filetype_to_int(filetype: &watchexec_events::FileType) -> usize {
    match filetype {
        watchexec_events::FileType::File => 1,
        watchexec_events::FileType::Dir => 2,
        watchexec_events::FileType::Symlink => 3,
        watchexec_events::FileType::Other => 4,
    }
}

impl Ord for FileType {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        filetype_to_int(&self.0).cmp(&filetype_to_int(&other.0))
    }
}

impl PartialOrd for FileType {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl TryFrom<Event> for Operation {
    type Error = ();

    fn try_from(value: Event) -> Result<Self, Self::Error> {
        const OTHER_FILE_EVENT: Tag = Tag::FileEventKind(FileEventKind::Other);

        if value.tags.contains(&OTHER_FILE_EVENT) {
            tracing::trace!("Skipped an event {value} because we are sure it is not relevant");

            return Err(());
        }

        let mut operation = OperationBuilder::create_empty();

        for tag in value.tags.iter() {
            match tag {
                Tag::Path { path, file_type } => {
                    if path
                        .file_name()
                        .map(|name| name == "4913")
                        .unwrap_or_default()
                    {
                        return Err(());
                    }

                    operation.path(path.clone());
                    operation.filetype(file_type.map(FileType));
                }
                Tag::FileEventKind(event_kind) => match event_kind {
                    FileEventKind::Any => todo!(),
                    FileEventKind::Access(_) => {
                        return Err(());
                    }
                    FileEventKind::Create(_) => {
                        operation.kind(OperationType::Create);
                    }
                    FileEventKind::Modify(_) => {
                        operation.kind(OperationType::Update);
                    }
                    FileEventKind::Remove(_) => {
                        operation.kind(OperationType::Delete);
                    }
                    FileEventKind::Other => todo!(),
                },
                Tag::Source(_) => {}
                tag => {
                    tracing::trace!("Ignored tag {tag:?}");
                }
            }
        }

        operation
            .build()
            .inspect_err(|err| tracing::error!("Could not infer operation from {value:?} | {err}"))
            .map_err(|_| ())
    }
}

pub async fn handle_events(events: Vec<Event>, args: Arc<Arguments>) {
    let operations = events
        .into_iter()
        .map(Operation::try_from)
        .filter_map(Result::ok)
        .map(|op| (op.path, (op.filetype, op.kind)))
        .collect::<BTreeMap<_, _>>();
    dbg!(&operations);

    for (path, (filetype, kind)) in operations.iter() {
        let filetype = filetype.map(|filetype| filetype.0);
        let path = match pathdiff::diff_paths(path, &args.root) {
            Some(path) => path,
            None => {
                tracing::warn!(
                    "Could not determine path difference between {path:?} and {:?}! Skipping...",
                    args.root
                );

                continue;
            }
        };
        let target = args.target.join(&path);

        match kind {
            OperationType::Create | OperationType::Update
                if matches!(filetype, Some(watchexec_events::FileType::Dir)) =>
            {
                tracing::info!("Creating directory at {target:?}");
                let _ = tokio::fs::create_dir_all(&target).await.inspect_err(|err| {
                    tracing::error!("Could not create directory at {target:?} ? {err}");
                });
            }
            OperationType::Create | OperationType::Update => {
                tracing::info!("Copying from {path:?} to {target:?}");
                let _ = tokio::fs::copy(&path, &target).await.inspect_err(|err| {
                    tracing::error!("Could not copy file from {path:?} to {target:?} ? {err}");
                });
            }
            OperationType::Delete => {
                tracing::info!("Deleting path {target:?}");
                let metadata = match tokio::fs::metadata(&target).await {
                    Ok(metadata) => metadata,
                    Err(err) => {
                        tracing::warn!("Could not get metadata for path {target:?} ? {err}");

                        continue;
                    }
                };

                let removal_result = if metadata.is_dir() {
                    tokio::fs::remove_dir_all(&target).await
                } else {
                    tokio::fs::remove_file(&target).await
                };

                if let Err(err) = removal_result {
                    tracing::warn!("Could not delete file at {target:?} {err}");
                }
            }
        }
    }
}
