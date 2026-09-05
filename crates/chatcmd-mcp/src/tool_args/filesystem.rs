tool_args!(ListV2Args {
    path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    limit: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sort: Option<chatcmd_runtime::FsListSort>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    metadata: Option<Vec<chatcmd_runtime::FsListMetadata>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    include_hidden: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    budget: Option<chatcmd_runtime::FsListBudget>
});
tool_args!(SearchArgs {
    path: String,
    query: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mode: Option<chatcmd_runtime::SearchMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    case_sensitive: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    word_boundary: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    include: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    exclude: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    include_ignored: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    context_before: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    context_after: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_matches_per_file: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    limit: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_results: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_file_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_snippet_bytes: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    budget: Option<chatcmd_runtime::FsSearchBudget>
});
tool_args!(FindArgs {
    path: String,
    pattern: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pattern_mode: Option<chatcmd_runtime::FindPatternMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    case_sensitive: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    entry_types: Option<Vec<chatcmd_runtime::FindEntryType>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_depth: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    include_ignored: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    include_hidden: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    exclude: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    extensions: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    limit: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_results: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    budget: Option<chatcmd_runtime::FsFindBudget>
});
tool_args!(ReadArgs {
    path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_characters: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    start_line: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    line_count: Option<usize>
});
#[derive(Debug, Clone, Default, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct ReadRangeArgs {
    unit: String,
    start: u64,
    limit: usize,
}
#[derive(Debug, Clone, Default, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct ReadBudgetArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_bytes_read: Option<u64>,
}
tool_args!(ReadV2Args {
    path: String,
    range: ReadRangeArgs,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_bytes: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    include_line_endings: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expected_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    budget: Option<ReadBudgetArgs>
});
tool_args!(BatchReadArgs {
    requests: Vec<chatcmd_runtime::TextReadRequestV2>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_items: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_total_output_bytes: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    concurrency: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    budget: Option<chatcmd_runtime::TextReadBudget>
});
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
enum ForbiddenContentSourceValue {}

#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(untagged)]
enum WriteTextSourceArgs {
    Inline {
        content: String,
        #[serde(
            default,
            rename = "contentRef",
            skip_serializing_if = "Option::is_none"
        )]
        #[schemars(rename = "contentRef")]
        content_ref: Option<ForbiddenContentSourceValue>,
    },
    Reference {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content: Option<ForbiddenContentSourceValue>,
        #[serde(rename = "contentRef")]
        #[schemars(rename = "contentRef")]
        content_ref: String,
    },
}

#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct WriteTextArgs {
    #[serde(flatten)]
    common: CommonToolArgs,
    path: String,
    #[serde(flatten)]
    source: WriteTextSourceArgs,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    overwrite: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expected_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    metadata_policy: Option<chatcmd_runtime::MetadataPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    durability: Option<chatcmd_runtime::DurabilityMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    require_atomic: Option<bool>,
}
tool_args!(ReplaceTextArgs {
    path: String,
    old_text: String,
    new_text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expected_occurrences: Option<usize>
});
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(untagged)]
enum ApplyEditsSourceArgs {
    Inline {
        edits: Vec<chatcmd_runtime::TextEdit>,
        #[serde(
            default,
            rename = "contentRef",
            skip_serializing_if = "Option::is_none"
        )]
        #[schemars(rename = "contentRef")]
        content_ref: Option<ForbiddenContentSourceValue>,
    },
    Reference {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        edits: Option<ForbiddenContentSourceValue>,
        #[serde(rename = "contentRef")]
        #[schemars(rename = "contentRef")]
        content_ref: String,
    },
}

#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct ApplyEditsArgs {
    #[serde(flatten)]
    common: CommonToolArgs,
    path: String,
    expected_version: String,
    coordinate_system: chatcmd_runtime::EditCoordinateSystem,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    column_encoding: Option<chatcmd_runtime::EditColumnEncoding>,
    #[serde(flatten)]
    source: ApplyEditsSourceArgs,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dry_run: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    preserve_line_endings: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    preserve_bom: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    budget: Option<chatcmd_runtime::ApplyEditsBudget>,
}

#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(untagged)]
enum WriteRawSourceArgs {
    Inline {
        base64: String,
        #[serde(
            default,
            rename = "contentRef",
            skip_serializing_if = "Option::is_none"
        )]
        #[schemars(rename = "contentRef")]
        content_ref: Option<ForbiddenContentSourceValue>,
    },
    Reference {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        base64: Option<ForbiddenContentSourceValue>,
        #[serde(rename = "contentRef")]
        #[schemars(rename = "contentRef")]
        content_ref: String,
    },
}

#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct WriteRawArgs {
    #[serde(flatten)]
    common: CommonToolArgs,
    path: String,
    #[serde(flatten)]
    source: WriteRawSourceArgs,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    overwrite: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expected_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    metadata_policy: Option<chatcmd_runtime::MetadataPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    durability: Option<chatcmd_runtime::DurabilityMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    require_atomic: Option<bool>,
}
tool_args!(BlobBeginArgs {
    purpose: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expected_size_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    content_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expected_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    chunk_size_bytes: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ttl_seconds: Option<u64>,
    #[serde(default)]
    budget: chatcmd_runtime::BlobToolBudget
});
tool_args!(BlobChunkArgs {
    upload_id: String,
    offset: u64,
    data_base64: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    chunk_sha256: Option<String>,
    #[serde(default)]
    budget: chatcmd_runtime::BlobToolBudget
});
tool_args!(BlobStatusArgs {
    upload_id: String,
    #[serde(default)]
    budget: chatcmd_runtime::BlobToolBudget
});
tool_args!(BlobSealArgs {
    upload_id: String,
    final_size_bytes: u64,
    sha256: String,
    #[serde(default)]
    budget: chatcmd_runtime::BlobToolBudget
});
