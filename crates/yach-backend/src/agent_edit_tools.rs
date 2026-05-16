use crate::{
    NativeEditHunk, NativeEditOperation, NativeEditPolicy, NativeEditTransactionRequest,
    NativeResourceRoot, NativeToolError, NativeToolRegistry, PendingNativeToolRequest,
    native_edit_read_existing_text, native_edit_sha256_hex,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedAgentEditToolRequest {
    pub transaction: NativeEditTransactionRequest,
    pub path: String,
    pub operation: String,
}

pub fn normalize_agent_edit_tool_request(
    registry: &NativeToolRegistry,
    root: &NativeResourceRoot,
    request: &PendingNativeToolRequest,
    edit_policy: NativeEditPolicy,
) -> Result<NormalizedAgentEditToolRequest, NativeToolError> {
    let definition = registry.validate_request_schema_only(request)?;

    match definition.name.as_str() {
        "edit_text_file" => normalize_edit_text_file(root, request, edit_policy),
        "create_text_file" => normalize_create_text_file(request),
        _ => Err(NativeToolError::UnknownTool),
    }
}

fn normalize_edit_text_file(
    root: &NativeResourceRoot,
    request: &PendingNativeToolRequest,
    edit_policy: NativeEditPolicy,
) -> Result<NormalizedAgentEditToolRequest, NativeToolError> {
    let path = string_argument(request, "path")?;
    let find = string_argument(request, "find")?;
    let replace = string_argument(request, "replace")?;
    let (path, text) = native_edit_read_existing_text(root, &path, &edit_policy)
        .map_err(|_| NativeToolError::MalformedArguments)?;
    let expected_sha256 = native_edit_sha256_hex(text.as_bytes());

    Ok(NormalizedAgentEditToolRequest {
        transaction: NativeEditTransactionRequest {
            operations: vec![NativeEditOperation::ModifyTextFile {
                path: path.clone(),
                expected_sha256,
                hunks: vec![NativeEditHunk { find, replace }],
            }],
        },
        path,
        operation: String::from("edit_text_file"),
    })
}

fn normalize_create_text_file(
    request: &PendingNativeToolRequest,
) -> Result<NormalizedAgentEditToolRequest, NativeToolError> {
    let path = string_argument(request, "path")?;
    let content = string_argument(request, "content")?;

    Ok(NormalizedAgentEditToolRequest {
        transaction: NativeEditTransactionRequest {
            operations: vec![NativeEditOperation::CreateTextFile {
                path: path.clone(),
                content,
            }],
        },
        path,
        operation: String::from("create_text_file"),
    })
}

fn string_argument(
    request: &PendingNativeToolRequest,
    field: &str,
) -> Result<String, NativeToolError> {
    request
        .arguments
        .as_object()
        .and_then(|arguments| arguments.get(field))
        .and_then(serde_json::Value::as_str)
        .map(String::from)
        .ok_or(NativeToolError::MalformedArguments)
}
