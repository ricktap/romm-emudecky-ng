//! Static descriptors for the ordered library sync plan.
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CollectionKind { User, Smart, Franchise }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum WorkUnit {
    Platform { id: i64, name: String, slug: String, rom_count: i64 },
    Collection { id: String, name: String, slug: String, rom_count: i64, collection_kind: CollectionKind },
}

impl WorkUnit {
    /// Query filter, kept here so every transport dispatches on the same kind.
    pub fn query_filter(&self) -> (&'static str, String) {
        match self {
            Self::Platform { id, .. } => ("platform_ids", id.to_string()),
            Self::Collection { id, collection_kind, .. } => (match collection_kind {
                CollectionKind::User => "collection_id", CollectionKind::Smart => "smart_collection_id",
                CollectionKind::Franchise => "virtual_collection_id",
            }, id.clone()),
        }
    }
}
impl WorkUnit {
    pub fn name(&self)->&str {match self{Self::Platform{name,..}|Self::Collection{name,..}=>name}}
    pub fn id_value(&self)->serde_json::Value {match self{Self::Platform{id,..}=>serde_json::json!(id),Self::Collection{id,..}=>serde_json::json!(id)}}
    pub fn count(&self)->i64 {match self{Self::Platform{rom_count,..}|Self::Collection{rom_count,..}=>*rom_count}}
}
