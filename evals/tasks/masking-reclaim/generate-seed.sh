#!/bin/bash
# Regenerates the committed, deterministic masking fixture. The codeword and
# chapter bytes originate here so the seed, oracle, and verifier cannot drift.
set -euo pipefail

root=$(cd "$(dirname "$0")" && pwd)
fixture="$root/fixture"
session_dir="$fixture/.yach/sessions"
notes_dir="$fixture/notes"
session_id="eval-masking"
codeword="juniper-4417-ember"
seed=4417
words=(
  amber basin cedar delta ember fjord granite harbor iris juniper kiln lagoon
  meadow north orchard prairie quartz river spruce terrace umber valley willow
)

mkdir -p "$session_dir" "$notes_dir"
rm -f "$notes_dir"/chapter-*.md "$session_dir/$session_id.jsonl"

next_word() {
  seed=$(((seed * 1103515245 + 12345) % 2147483648))
  generated_word="${words[$((seed % ${#words[@]}))]}"
}

make_chapter() {
  chapter="$1"
  destination="$notes_dir/chapter-$chapter.md"
  {
    printf '# Chapter %s\n\n' "$chapter"
    if [ "$chapter" -eq 1 ]; then
      printf 'CODEWORD: %s\n\n' "$codeword"
    fi
    section=1
    while [ "$section" -le 28 ]; do
      next_word
      place="$generated_word"
      next_word
      material="$generated_word"
      next_word
      reading="$generated_word"
      printf 'Section %s.%s records the %s survey team comparing %s samples at the %s station, retaining calibration notes, cross-check observations, and the complete field narrative for later review without editorial compression.\n\n' \
        "$chapter" "$section" "$place" "$material" "$reading"
      section=$((section + 1))
    done
  } > "$destination"
}

append_turn() {
  chapter="$1"
  chapter_path="$notes_dir/chapter-$chapter.md"
  bytes=$(wc -c < "$chapter_path" | tr -d ' ')
  user_id="entry-$chapter-user"
  assistant_id="entry-$chapter-assistant"
  request_id="tool-request-$chapter"
  turn_id="turn-$chapter"
  if [ "$chapter" -eq 1 ]; then
    parent_id=""
  else
    parent_id="entry-$((chapter - 1))-assistant"
  fi

  {
    jq -nc \
      --arg session_id "$session_id" --arg entry_id "$user_id" --arg parent_id "$parent_id" \
      --arg turn_id "$turn_id" --arg chapter "$chapter" \
      '{type:"entry_appended", session_id:$session_id, entry_id:$entry_id,
        parent_entry_id:(if $parent_id == "" then null else $parent_id end), turn_id:$turn_id,
        role:"user", text:("Read notes/chapter-" + $chapter + ".md and summarize it in one sentence."), provider:null}'
    jq -nc \
      --arg session_id "$session_id" --arg turn_id "$turn_id" --arg request_id "$request_id" \
      --arg chapter "$chapter" \
      '{type:"tool_request_recorded", session_id:$session_id, turn_id:$turn_id,
        tool_request_id:$request_id, tool_name:"read_text_file", provider_call_id:null,
        validation:{Ok:null}, permission:"allowed",
        argument_summary:{summary:"tool payload redacted", byte_count:29, redacted:true, truncated:false},
        argument_content:({path:("notes/chapter-" + $chapter + ".md")} | tojson)}'
    jq -nc \
      --arg session_id "$session_id" --arg turn_id "$turn_id" --arg request_id "$request_id" \
      --argjson bytes "$bytes" --rawfile result "$chapter_path" \
      '{type:"tool_execution_finished", session_id:$session_id, turn_id:$turn_id,
        tool_request_id:$request_id, outcome:"completed", reason:null,
        result_summary:{summary:"read_text_file result redacted", byte_count:$bytes, redacted:true, truncated:false},
        result_content:$result}'
    jq -nc \
      --arg session_id "$session_id" --arg entry_id "$assistant_id" --arg parent_id "$user_id" \
      --arg turn_id "$turn_id" --arg chapter "$chapter" \
      '{type:"entry_appended", session_id:$session_id, entry_id:$entry_id, parent_entry_id:$parent_id,
        turn_id:$turn_id, role:"assistant", text:("Chapter " + $chapter + " was read and summarized."), provider:null}'
    jq -nc --arg session_id "$session_id" --arg turn_id "$turn_id" \
      '{type:"turn_finished", session_id:$session_id, turn_id:$turn_id, outcome:"completed", reason:null}'
  } >> "$session_dir/$session_id.jsonl"
}

chapter=1
while [ "$chapter" -le 8 ]; do
  make_chapter "$chapter"
  append_turn "$chapter"
  chapter=$((chapter + 1))
done

printf 'Generated %s with codeword %s.\n' "$session_dir/$session_id.jsonl" "$codeword"
