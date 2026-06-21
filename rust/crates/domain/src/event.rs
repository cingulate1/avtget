use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactStatusValue {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
    NotRequested,
    Skipped,
    Warning,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ArtifactKind {
    #[serde(rename = "video")]
    Video,
    #[serde(rename = "audio")]
    Audio,
    #[serde(rename = "transcript")]
    Transcript,
    #[serde(rename = "summary")]
    Summary,
    #[serde(rename = "__filestem__")]
    Filestem,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum BackendEvent {
    #[serde(rename = "log")]
    Log { message: String },
    #[serde(rename = "progress")]
    Progress { percent: f64, stage: String },
    #[serde(rename = "status_change")]
    StatusChange { item_id: String, status: JobStatus },
    #[serde(rename = "stage_count")]
    StageCount {
        stage_name: String,
        current: i64,
        total: i64,
    },
    #[serde(rename = "artifact_status")]
    ArtifactStatus {
        item_id: String,
        artifact: ArtifactKind,
        status: String,
    },
    #[serde(rename = "job_finished")]
    JobFinished { summary: String },
    #[serde(rename = "job_error")]
    JobError { error: String },
}
