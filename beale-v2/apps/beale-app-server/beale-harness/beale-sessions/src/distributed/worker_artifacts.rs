use super::*;
use beale_contracts::swarm_files::{Artifact, MaterializeArtifact};
use beale_storage::swarm_files::{hash, materialize_worker_artifact, stage_worker_artifact};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

const CHUNK: usize = 192 * 1024;

impl Worker<'_> {
    pub(super) async fn bounded_outcome(
        &self,
        call: &InferenceCall,
        mut result: Value,
    ) -> Result<Value> {
        if serde_json::to_vec(&result)?.len() <= MAX_DOCUMENT_BYTES / 4 {
            return Ok(result);
        }
        let output = result["output"]
            .as_str()
            .context("large tool outcome has no text")?
            .to_owned();
        let preview = output[..output.floor_char_boundary(32_768)].to_owned();
        let workspace = self.launch.workspace.clone();
        let operation =
            hash(format!("{}:{}:output", self.launch.identity.attempt_id, call.id).as_bytes());
        let path = tokio::task::spawn_blocking(move || {
            beale_storage::swarm_files::retain_worker_output(&workspace, &operation, &output)
        })
        .await??;
        result["output"] = json!(format!("{preview}\n\nFull output retained at file:{path}."));
        result["candidate_output"] = json!(path);
        Ok(result)
    }

    pub(super) async fn publish_outcome(
        &self,
        call: &InferenceCall,
        mut result: Value,
    ) -> Result<Value> {
        if let Some(source) = result["candidate_output"].as_str() {
            let operation = hash(
                format!(
                    "{}:{}:full-output",
                    self.launch.identity.attempt_id, call.id
                )
                .as_bytes(),
            );
            let id = self.publish_artifact(&operation, source).await?;
            result["output"] = json!(format!(
                "{}\nAccepted full-output artifact: {id}",
                result["output"]
                    .as_str()
                    .context("invalid retained output")?
            ));
            result["artifact"] = json!(id);
        }
        Ok(result)
    }

    pub(super) async fn publish_artifact(
        &self,
        operation: &str,
        source_path: &str,
    ) -> Result<String> {
        self.deadline()?;
        let workspace = self.launch.workspace.clone();
        let recovery = self.launch.recovery.clone();
        let operation = operation.to_owned();
        let source_path = source_path.to_owned();
        let key = hash(operation.as_bytes());
        let (upload, source) = tokio::task::spawn_blocking(move || {
            stage_worker_artifact(&workspace, &recovery, &operation, &source_path)
        })
        .await??;
        let begin = format!("artifact-begin:{key}");
        let progress: TransferProgress = serde_json::from_value(
            self.remote(self.request(
                begin.clone(),
                WorkerOperation::ArtifactBegin {
                    artifact: upload.clone(),
                },
            ))
            .await?,
        )?;
        let mut offset = progress.next_offset;
        ensure!(offset <= upload.size, "invalid publication progress");
        let mut file = tokio::fs::File::open(source).await?;
        file.seek(std::io::SeekFrom::Start(offset)).await?;
        while offset < upload.size {
            let mut bytes = vec![0; usize::try_from((upload.size - offset).min(CHUNK as u64))?];
            file.read_exact(&mut bytes).await?;
            let id = format!("artifact-chunk:{key}:{offset}");
            let result: TransferProgress = serde_json::from_value(
                self.remote(self.request(
                    id.clone(),
                    WorkerOperation::ArtifactChunk {
                        transfer_id: upload.id.clone(),
                        offset,
                        sha256: hash(&bytes),
                        bytes: bytes.clone(),
                    },
                ))
                .await?,
            )?;
            ensure!(
                result.next_offset == offset + bytes.len() as u64,
                "publication offset changed"
            );
            offset = result.next_offset;
            self.spool.acknowledge(&id)?;
        }
        let seal = format!("artifact-seal:{key}");
        let artifact: Artifact = serde_json::from_value(
            self.remote(self.request(
                seal.clone(),
                WorkerOperation::ArtifactSeal {
                    transfer_id: upload.id.clone(),
                },
            ))
            .await?,
        )?;
        ensure!(
            artifact.id == upload.id
                && artifact.sha256 == upload.sha256
                && artifact.size == upload.size
                && artifact.source_path == upload.source_path
                && artifact.executable == upload.executable,
            "accepted artifact differs from captured publication"
        );
        self.spool.acknowledge(&begin)?;
        self.spool.acknowledge(&seal)?;
        Ok(artifact.id)
    }

    pub(super) async fn attachments(
        &self,
        call: &InferenceCall,
        arguments: &mut Value,
    ) -> Result<()> {
        fn references(
            value: &Value,
            pointer: &str,
            found: &mut Vec<(String, String)>,
        ) -> Result<()> {
            match value {
                Value::Object(object) => {
                    if object.get("kind").and_then(Value::as_str) == Some("artifact") {
                        let reference = object
                            .get("reference")
                            .and_then(Value::as_str)
                            .context("artifact reference missing")?;
                        if let Some(path) = reference.strip_prefix("file:") {
                            found.push((format!("{pointer}/reference"), path.into()));
                        }
                    }
                    for (key, value) in object {
                        let key_path =
                            format!("{pointer}/{}", key.replace('~', "~0").replace('/', "~1"));
                        if matches!(
                            key.as_str(),
                            "artifact_refs" | "poc_artifact_refs" | "patch_refs" | "test_refs"
                        ) {
                            for (index, reference) in value
                                .as_array()
                                .context("artifact references must be an array")?
                                .iter()
                                .enumerate()
                            {
                                let reference = reference
                                    .as_str()
                                    .context("artifact reference must be a string")?;
                                if let Some(path) = reference.strip_prefix("file:") {
                                    found.push((format!("{key_path}/{index}"), path.into()));
                                }
                            }
                        } else {
                            references(value, &key_path, found)?;
                        }
                    }
                }
                Value::Array(values) => {
                    for (index, value) in values.iter().enumerate() {
                        references(value, &format!("{pointer}/{index}"), found)?;
                    }
                }
                _ => (),
            }
            Ok(())
        }
        let mut found = Vec::new();
        references(arguments, "", &mut found)?;
        let mut staged = arguments.clone();
        for (pointer, source) in found {
            let operation = hash(
                format!(
                    "{}:{}:{}:{pointer}",
                    self.launch.identity.scan_id, self.launch.identity.attempt_id, call.id
                )
                .as_bytes(),
            );
            let id = self.publish_artifact(&operation, &source).await?;
            *staged
                .pointer_mut(&pointer)
                .context("artifact reference disappeared")? = json!(id);
        }
        *arguments = staged;
        Ok(())
    }

    pub(super) async fn materialize(
        &self,
        call: &InferenceCall,
        arguments: &Value,
    ) -> Result<Value> {
        let request: MaterializeArtifact = serde_json::from_value(arguments.clone())?;
        ensure!(
            beale_contracts::swarm_files::relative_path(&request.destination),
            "invalid artifact destination"
        );
        let key = hash(format!("{}:{}", self.launch.identity.attempt_id, call.id).as_bytes());
        let path = self.launch.recovery.join(format!("download-{key}"));
        let mut file = tokio::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .await?;
        let mut offset = 0;
        let mut metadata = None;
        loop {
            let id = format!("artifact-read:{key}:{offset}");
            let chunk: ArtifactChunkResult = serde_json::from_value(
                self.remote(self.request(
                    id.clone(),
                    WorkerOperation::ArtifactRead {
                        artifact_id: request.artifact.clone(),
                        offset,
                        length: CHUNK as u32,
                    },
                ))
                .await?,
            )?;
            ensure!(
                hash(&chunk.bytes) == chunk.sha256
                    && metadata.as_ref().is_none_or(|old| old == &chunk.artifact),
                "downloaded artifact chunk changed"
            );
            file.write_all(&chunk.bytes).await?;
            file.sync_all().await?;
            offset += chunk.bytes.len() as u64;
            self.spool.acknowledge(&id)?;
            let complete = offset == chunk.artifact.size;
            metadata = Some(chunk.artifact);
            if complete {
                break;
            }
        }
        drop(file);
        let artifact = metadata.context("download metadata missing")?;
        self.deadline()?;
        let workspace = self.launch.workspace.clone();
        let destination = request.destination.clone();
        tokio::task::spawn_blocking(move || {
            materialize_worker_artifact(&workspace, &destination, &path, &artifact)
        })
        .await??;
        Ok(
            json!({"artifact":request.artifact,"destination":request.destination,"materialized":true}),
        )
    }
}
