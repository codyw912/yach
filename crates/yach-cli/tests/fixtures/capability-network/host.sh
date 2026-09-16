#!/bin/sh
# Fixture host for the extension capability contract proof.
# Speaks yach.extension-host.v2 over stdio JSONL. Not a sandbox: it
# never opens a socket; the uses_network declaration is consent only.
while IFS= read -r line; do
  case "$line" in
    *extension.initialize*)
      printf '%s\n' \
        '{"type":"extension.ready","protocol":"yach.extension-host.v2","extension_id":"example.capability-network"}' \
        '{"type":"tool.register","name":"fetch_url","description":"Return a static fixture payload for a URL.","risk":"uses_network","provider_visible":true,"input_schema":{"type":"object","additionalProperties":false,"required":["url"],"properties":{"url":{"type":"string"}},"maxSerializedBytes":1024}}'
      ;;
    *tool.invoke*)
      request_id=$(printf '%s\n' "$line" | sed -n 's/.*"request_id":"\([^"]*\)".*/\1/p')
      printf '%s\n' "{\"type\":\"tool.result\",\"request_id\":\"${request_id}\",\"content\":\"{\\\"ok\\\":true}\"}"
      ;;
  esac
done
