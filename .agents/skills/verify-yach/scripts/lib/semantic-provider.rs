//! Credless local Responses protocol fixture. Rust std + the skill's existing jq.
//! Replies are conditional on actual request context, never a FIFO text script.
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

const WIRE_CHECK: &str = r#"
def text: if (.content | type) == "string" then .content else [.content[]? | .text? // empty] | join("") end;
def user($s): any(.input[]?; .role == "user" and text == $s);
def assistant($s): any(.input[]?; .role == "assistant" and text == $s);
$f[0] as $f |
if .model != "gpt-fixture" or .stream != true or .store != false or (.input | type) != "array"
then error("expected streaming, non-storing gpt-fixture Responses request")
elif $mode == "resume" and $stage == 0 then
  if user($f.first_prompt) and (user($f.continuation_prompt) | not)
  then true else error("initial resume request lacks the actual first prompt") end
elif $mode == "resume" and $stage == 1 then
  [.input[] | {role, text: text}] as $messages |
  ([$messages | to_entries[] | select(.value.role == "user" and .value.text == $f.first_prompt) | .key]) as $first |
  ([$messages | to_entries[] | select(.value.role == "assistant" and .value.text == $f.first_reply) | .key]) as $reply |
  ([$messages | to_entries[] | select(.value.role == "user" and .value.text == $f.continuation_prompt) | .key]) as $next |
  if ($first | length) == 1 and ($reply | length) == 1 and ($next | length) == 1 and
     $first[0] < $reply[0] and $reply[0] < $next[0]
  then true else error("resumed request must contain the original user/assistant exchange before the new question") end
elif $mode == "compaction" and $stage == 0 then
  if user($f.first_prompt) then true else error("initial request lacks the standing instruction and foldable original") end
elif $mode == "compaction" and $stage == 1 then
  if user($f.first_prompt) and assistant($f.first_reply) and user($f.buffer_prompt)
  then true else error("buffer request lacks the initial exchange or kept-tail prompt") end
elif $mode == "compaction" and $stage == 2 then
  ([.input[]? | select(.role == "user") | text] | join("\n")) as $prompt |
  if ($prompt | contains("You are summarizing the earlier part of a coding session")) and
     ($prompt | contains("<conversation>")) and
     ($prompt | contains($f.first_prompt)) and ($prompt | contains($f.instruction)) and
     ($prompt | contains($f.folded_marker)) and ($prompt | contains($f.buffer_prompt)) and
     ($prompt | contains($f.continuation_prompt) | not)
  then true else error("summary request must contain the folded original standing instruction, not the continuation") end
elif $mode == "compaction" and $stage == 3 then
  (.instructions // "") as $instructions |
  if ($instructions | contains("Earlier work in this session was compacted.")) and
     ($instructions | contains($f.summary_marker)) and ($instructions | contains($f.instruction)) and
     ($instructions | contains($f.folded_marker) | not) and user($f.continuation_prompt) and
     assistant($f.buffer_reply) and
     (any(.input[]?; text | contains($f.instruction) or contains($f.folded_marker)) | not)
  then true else error("continuation requires the compacted standing instruction in system instructions and excludes folded original input") end
else error("unexpected provider request after the scenario's final stage") end
"#;

const EVIDENCE_CHECK: &str = r#"
def require($ok; $why): if $ok then . else error($why) end;
def turn_success($events; $turn; $request):
  ([$events[] | select(.type == "provider_attempt_finished" and .purpose == "turn" and .turn_id == $turn)] as $attempts |
   ($attempts | length) == 1 and $attempts[0].outcome == "succeeded" and
   $attempts[0].retry_index == 0 and $attempts[0].model == "gpt-fixture" and
   $attempts[0].provider_request_id == $request.request_id and $request.body.model == "gpt-fixture") and
  ([$events[] | select(.type == "turn_finished" and .turn_id == $turn)] as $finishes |
   ($finishes | length) == 1 and $finishes[0].outcome == "completed");
$f[0] as $f | $before as $old | $events as $all |
($all[($old | length):]) as $new |
([$all[] | .session_id? // empty] | unique) as $ids |
require(($ids | length) == 1; "events must retain one session identity") |
require($all[0:($old | length)] == $old; "pre-action session prefix was rewritten") |
require(all($wire[]; .accepted == true); "loopback fixture rejected an actual request") |
([$wire[] | select(.method == "POST")]) as $posts |
require(($posts | length) == (if $mode == "resume" then 2 else 4 end); "wrong number of actual provider requests") |
require([$posts[].stage] == (if $mode == "resume" then [0,1] else [0,1,2,3] end); "provider stages were missing or replayed") |
require(all($posts[]; .path == "/v1/responses"); "provider request used an unexpected endpoint") |
([$old[] | select(.type == "entry_appended" and .role == "user" and .text == $f.first_prompt)]) as $first |
require(($first | length) == 1 and turn_success($old; $first[0].turn_id; $posts[0]); "first turn must complete through its correlated provider request") |
require(any($old[]; .type == "entry_appended" and .turn_id == $first[0].turn_id and .role == "assistant" and .text == $f.first_reply); "first provider reply missing") |
([$new[] | select(.type == "entry_appended" and .role == "user" and .text == $f.continuation_prompt)]) as $next |
require(($next | length) == 1 and turn_success($new; $next[0].turn_id; $posts[-1]); "new continuation lacks its own correlated successful provider attempt and completed turn") |
require(all($old[]; .turn_id? != $next[0].turn_id); "continuation reused a pre-existing turn identity") |
([$new[] | select(.type == "entry_appended" and .turn_id == $next[0].turn_id and .role == "assistant")]) as $replies |
require(($replies | length) == 1 and $replies[0].text == $f.continuation_reply and
  ($replies[0].text | contains("fixture response:") | not); "exactly one conditional provider continuation reply required") |
require(($next[0].entry_id | type) == "string" and ($next[0].entry_id | length) > 0 and
  ($replies[0].entry_id | type) == "string" and ($replies[0].entry_id | length) > 0 and
  $next[0].entry_id != $replies[0].entry_id and $replies[0].parent_entry_id == $next[0].entry_id and
  all($old[] | select(.type == "entry_appended"); .entry_id != $next[0].entry_id and .entry_id != $replies[0].entry_id);
  "continuation entry identities must be fresh and the reply must parent to its new user entry") |
if $mode == "resume" then
  require(([$old[] | select(.type == "compaction_checkpoint")] | length) == 0; "resume scenario unexpectedly compacted") |
  require(([$new[] | select(.type == "provider_attempt_finished" and .purpose == "turn")] | length) == 1; "resume must append exactly one new provider turn attempt")
else
  ([$old[] | select(.type == "entry_appended" and .role == "user" and .text == $f.buffer_prompt)]) as $buffer |
  require(($buffer | length) == 1 and turn_success($old; $buffer[0].turn_id; $posts[1]); "kept buffer turn lacks its correlated successful provider request") |
  require(([$old[] | select(.type == "compaction_checkpoint")] | length) == 0; "checkpoint existed before the manual action") |
  ([$new | to_entries[] | select(.value.type == "compaction_checkpoint")]) as $checkpoints |
  require(($checkpoints | length) == 1; "manual /compact must persist exactly one NEW checkpoint; no-op is not success") |
  $checkpoints[0].value as $checkpoint |
  require($checkpoint.reason == "manual" and $checkpoint.compactor == "summary" and
    ($checkpoint.summary | contains($f.instruction)) and ($checkpoint.summary | contains($f.summary_marker)) and
    ($checkpoint.summary | contains($f.folded_marker) | not); "checkpoint must retain the standing instruction, not the folded original") |
  ([$old | to_entries[] | select(.value.type == "entry_appended" and .value.entry_id == $checkpoint.first_kept_entry_id)]) as $kept |
  require(($kept | length) == 1 and $kept[0].value.text == $f.buffer_reply; "checkpoint must point to the real kept tail, not original folded history") |
  ([$new | to_entries[] | select(.value.type == "provider_attempt_finished" and .value.purpose == "compaction_summary")]) as $summary |
  require(($summary | length) == 1 and $summary[0].value.outcome == "succeeded" and $summary[0].value.retry_index == 0 and
    $summary[0].value.model == "gpt-fixture" and $summary[0].value.provider_request_id == $posts[2].request_id and
    $posts[2].body.model == "gpt-fixture" and $summary[0].value.turn_id == $checkpoint.turn_id and $summary[0].key < $checkpoints[0].key;
    "checkpoint requires a correlated successful summary attempt before it") |
  ([$new | to_entries[] | select(.value.type == "entry_appended" and .value.role == "user" and .value.turn_id == $next[0].turn_id)][0].key) as $continued |
  require($continued > $checkpoints[0].key; "continuation did not occur after the NEW checkpoint")
end | {ok: true, mode: $mode, session_id: $ids[0], requests: ($posts | length), new_events: ($new | length)}
"#;

fn jq(args: &[&str], input: Option<&[u8]>) -> Result<Vec<u8>, String> {
    let mut child = Command::new("jq")
        .args(args)
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("starting jq: {error}"))?;
    if let Some(input) = input {
        child.stdin.take().ok_or("jq stdin unavailable")?.write_all(input).map_err(|error| error.to_string())?;
    }
    let output = child.wait_with_output().map_err(|error| error.to_string())?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}

fn path_str(path: &Path) -> Result<&str, String> {
    path.to_str().ok_or_else(|| format!("non-UTF8 fixture path: {}", path.display()))
}

fn check_wire(mode: &str, stage: usize, config: &Path, body: &Path) -> Result<(), String> {
    jq(&["-e", "--slurpfile", "f", path_str(config)?, "--arg", "mode", mode,
        "--argjson", "stage", &stage.to_string(), WIRE_CHECK, path_str(body)?], None).map(|_| ())
}

fn reply(config: &Path, mode: &str, stage: usize) -> Result<String, String> {
    let key = match (mode, stage) {
        (_, 0) => ".first_reply",
        ("resume", 1) | ("compaction", 3) => ".continuation_reply",
        ("compaction", 1) => ".buffer_reply",
        ("compaction", 2) => ".summary",
        _ => return Err(String::from("no response for unexpected stage")),
    };
    let output = jq(&["-er", key, path_str(config)?], None)?;
    Ok(String::from_utf8(output).map_err(|error| error.to_string())?.trim_end_matches('\n').to_owned())
}

struct Request {
    method: String,
    path: String,
    body: Vec<u8>,
}

fn read_request(stream: &mut TcpStream) -> Result<Request, String> {
    stream.set_read_timeout(Some(Duration::from_secs(5))).map_err(|error| error.to_string())?;
    stream.set_write_timeout(Some(Duration::from_secs(5))).map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    let mut chunk = [0; 8192];
    let header_end = loop {
        if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break index;
        }
        if bytes.len() > 65_536 { return Err(String::from("HTTP headers exceed 64 KiB")); }
        let read = stream.read(&mut chunk).map_err(|error| error.to_string())?;
        if read == 0 { return Err(String::from("incomplete HTTP headers")); }
        bytes.extend_from_slice(&chunk[..read]);
    };
    let header = std::str::from_utf8(&bytes[..header_end]).map_err(|error| error.to_string())?;
    let mut request_line = header.lines().next().ok_or("missing request line")?.split_whitespace();
    let method = request_line.next().ok_or("missing HTTP method")?.to_owned();
    let path = request_line.next().ok_or("missing HTTP path")?.to_owned();
    let mut length = None;
    for line in header.lines().skip(1) {
        let (name, value) = line.split_once(':').ok_or("invalid HTTP header")?;
        if name.eq_ignore_ascii_case("transfer-encoding") { return Err(String::from("chunked requests are not supported by this fixture")); }
        if name.eq_ignore_ascii_case("content-length") {
            length = Some(value.trim().parse::<usize>().map_err(|error| error.to_string())?);
        }
    }
    let length = length.unwrap_or(0);
    if length > 2_000_000 { return Err(String::from("HTTP request exceeds 2 MB")); }
    let body_start = header_end + 4;
    while bytes.len() < body_start + length {
        let read = stream.read(&mut chunk).map_err(|error| error.to_string())?;
        if read == 0 { return Err(String::from("incomplete HTTP body")); }
        bytes.extend_from_slice(&chunk[..read]);
    }
    Ok(Request { method, path, body: bytes.split_off(body_start) })
}

fn respond(stream: &mut TcpStream, status: &str, content_type: &str, request_id: &str, body: &[u8]) -> io::Result<()> {
    write!(stream, "HTTP/1.1 {status}\r\nx-request-id: {request_id}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len())?;
    stream.write_all(body)
}

fn sse(reply: &str, request_id: &str) -> Result<Vec<u8>, String> {
    jq(&["-nr", "--arg", "text", reply, "--arg", "id", request_id, r#"
      "data: " + ({type:"response.output_text.delta",item_id:("msg-"+$id),output_index:0,content_index:0,sequence_number:1,delta:$text}|tojson) + "\n\n" +
      "data: " + ({type:"response.completed",sequence_number:2,response:{id:("resp-"+$id),object:"response",created_at:0,status:"completed",error:null,incomplete_details:null,instructions:null,max_output_tokens:null,model:"gpt-fixture",usage:{input_tokens:1,input_tokens_details:{cached_tokens:0},output_tokens:1,output_tokens_details:{reasoning_tokens:0},total_tokens:2},output:[{type:"message",id:("msg-"+$id),status:"completed",role:"assistant",content:[{type:"output_text",annotations:[],text:$text}]}],tools:[]}}|tojson) + "\n\ndata: [DONE]\n\n"
    "#], None)
}

fn log_request(log: &mut File, request: &Request, body: &Path, stage: usize, accepted: bool, error: &str, id: &str) -> Result<(), String> {
    let line = jq(&["-cn", "--arg", "method", &request.method, "--arg", "path", &request.path,
        "--argjson", "stage", &stage.to_string(), "--argjson", "accepted", if accepted { "true" } else { "false" },
        "--arg", "error", error, "--arg", "request_id", id, "--rawfile", "raw", path_str(body)?,
        r#"{method:$method,path:$path,stage:$stage,accepted:$accepted,error:$error,request_id:$request_id,body:(try ($raw|fromjson) catch $raw)}"#], None)?;
    log.write_all(&line).and_then(|_| log.flush()).map_err(|error| error.to_string())
}

fn serve(mode: &str, dir: &Path) -> Result<(), String> {
    let config = dir.join("config.json");
    let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|error| error.to_string())?;
    listener.set_nonblocking(true).map_err(|error| error.to_string())?;
    let address = listener.local_addr().map_err(|error| error.to_string())?;
    let mut log = OpenOptions::new().create_new(true).write(true).open(dir.join("requests.jsonl")).map_err(|error| error.to_string())?;
    fs::write(dir.join("ready.tmp"), format!("http://{address}/v1\n")).map_err(|error| error.to_string())?;
    fs::rename(dir.join("ready.tmp"), dir.join("ready")).map_err(|error| error.to_string())?;
    let expected = if mode == "resume" { 2 } else { 4 };
    let mut stage = 0;
    let mut count = 0;
    let mut rejections = 0;
    while !dir.join("stop").exists() {
        let (mut stream, _) = match listener.accept() {
            Ok(connection) => connection,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => { thread::sleep(Duration::from_millis(50)); continue; }
            Err(error) => return Err(error.to_string()),
        };
        count += 1;
        let request = match read_request(&mut stream) {
            Ok(request) => request,
            Err(error) => {
                rejections += 1;
                eprintln!("request {count}: {error}");
                respond(&mut stream, "400 Bad Request", "application/json", "local-invalid", b"{\"error\":{\"message\":\"invalid fixture HTTP request\"}}") .map_err(|error| error.to_string())?;
                continue;
            }
        };
        let body = dir.join(format!("request-{count:02}.json"));
        fs::write(&body, &request.body).map_err(|error| error.to_string())?;
        let id = format!("local-{mode}-{stage}");
        if request.method == "GET" && request.path == "/v1/models" {
            log_request(&mut log, &request, &body, stage, true, "", &id)?;
            respond(&mut stream, "200 OK", "application/json", &id, b"{\"object\":\"list\",\"data\":[{\"id\":\"gpt-fixture\",\"object\":\"model\",\"created\":0,\"owned_by\":\"local-protocol-fixture\"}]}").map_err(|error| error.to_string())?;
            continue;
        }
        let validation = if request.method == "POST" && request.path == "/v1/responses" {
            check_wire(mode, stage, &config, &body)
        } else { Err(format!("unexpected route: {} {}", request.method, request.path)) };
        match validation {
            Ok(()) => {
                let response = sse(&reply(&config, mode, stage)?, &id)?;
                log_request(&mut log, &request, &body, stage, true, "", &id)?;
                respond(&mut stream, "200 OK", "text/event-stream", &id, &response).map_err(|error| error.to_string())?;
                stage += 1;
            }
            Err(error) => {
                rejections += 1;
                eprintln!("request {count}, stage {stage}: {error}");
                log_request(&mut log, &request, &body, stage, false, &error, &id)?;
                respond(&mut stream, "400 Bad Request", "application/json", &id, b"{\"error\":{\"message\":\"local semantic request validation failed\",\"type\":\"invalid_request_error\"}}") .map_err(|error| error.to_string())?;
            }
        }
    }
    let result = jq(&["-cn", "--arg", "mode", mode, "--argjson", "stages", &stage.to_string(),
        "--argjson", "expected", &expected.to_string(), "--argjson", "rejections", &rejections.to_string(),
        "{ok:($stages == $expected and $rejections == 0),mode:$mode,accepted_stages:$stages,expected_stages:$expected,rejections:$rejections}"], None)?;
    fs::write(dir.join("result.json"), result).map_err(|error| error.to_string())?;
    if stage == expected && rejections == 0 { Ok(()) } else { Err(format!("incomplete/invalid protocol proof: {stage}/{expected} stages, {rejections} rejections")) }
}

fn check_evidence(mode: &str, dir: &Path, before: &Path, session: &Path) -> Result<(), String> {
    let config = dir.join("config.json");
    let requests = dir.join("requests.jsonl");
    let wire = jq(&["-sc", ".", path_str(&requests)?], None)?;
    let old = jq(&["-sc", ".", path_str(before)?], None)?;
    let events = jq(&["-sc", ".", path_str(session)?], None)?;
    let output = jq(&["-n", "--arg", "mode", mode, "--slurpfile", "f", path_str(&config)?,
        "--argjson", "before", std::str::from_utf8(&old).map_err(|error| error.to_string())?,
        "--argjson", "events", std::str::from_utf8(&events).map_err(|error| error.to_string())?,
        "--argjson", "wire", std::str::from_utf8(&wire).map_err(|error| error.to_string())?, EVIDENCE_CHECK], None)?;
    // Re-evaluate actual captured wire bodies: a forged accepted flag is not proof.
    let posts = jq(&["-sc", "map(select(.method == \"POST\")) | .[].body", path_str(&requests)?], None)?;
    for (stage, body) in posts.split(|byte| *byte == b'\n').filter(|line| !line.is_empty()).enumerate() {
        jq(&["-e", "--slurpfile", "f", path_str(&config)?, "--arg", "mode", mode,
            "--argjson", "stage", &stage.to_string(), WIRE_CHECK], Some(body))?;
    }
    io::stdout().write_all(&output).map_err(|error| error.to_string())
}

fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args().collect();
    let command = args.get(1).ok_or("usage: semantic-provider serve MODE DIR | check MODE STAGE CONFIG BODY | check-evidence MODE DIR BEFORE SESSION")?;
    let mode = args.get(2).ok_or("missing scenario mode")?;
    if !matches!(mode.as_str(), "resume" | "compaction") { return Err(String::from("mode must be resume or compaction")); }
    match (command.as_str(), args.len()) {
        ("serve", 4) => serve(mode, &PathBuf::from(&args[3])),
        ("check", 6) => check_wire(mode, args[3].parse::<usize>().map_err(|error| error.to_string())?, Path::new(&args[4]), Path::new(&args[5])),
        ("check-evidence", 6) => check_evidence(mode, Path::new(&args[3]), Path::new(&args[4]), Path::new(&args[5])),
        _ => Err(String::from("invalid fixture arguments")),
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("semantic-provider: {error}");
        std::process::exit(1);
    }
}
