"""Deterministic stdio fixture; each received message records its text mirror."""
import json
import os
from pathlib import Path
import subprocess
import sys
import time

scenario, transcript = sys.argv[1:3]
documents = {}
request_attempts = {}
encoding = "utf-8" if scenario == "utf8" else "utf-16"


def send(value):
    body = json.dumps(dict(jsonrpc="2.0", **value), ensure_ascii=False).encode()
    sys.stdout.buffer.write(f"Content-Length: {len(body)}\r\n\r\n".encode() + body)
    sys.stdout.buffer.flush()


def offset(text, position):
    lines = text.splitlines(keepends=True)
    if not lines or text.endswith(("\n", "\r")):
        lines.append("")
    row, units = position["line"], position["character"]
    at = sum(map(len, lines[:row]))
    count = 0
    for character in lines[row].rstrip("\r\n"):
        if count == units:
            return at
        count += len(character.encode()) if encoding == "utf-8" else len(character.encode("utf-16-le")) // 2
        at += 1
    assert count == units, (count, units)
    return at


while True:
    headers = {}
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            sys.exit(0)
        if line == b"\r\n":
            break
        key, value = line.decode().split(":", 1)
        headers[key.lower()] = value.strip()
    request = json.loads(sys.stdin.buffer.read(int(headers["content-length"])))
    method, params = request.get("method"), request.get("params") or {}
    uri = params.get("textDocument", {}).get("uri")
    request_attempts[method] = request_attempts.get(method, 0) + 1
    retryable = method in ("textDocument/hover", "textDocument/definition", "textDocument/completion", "completionItem/resolve") and scenario in ("transient", "always-transient")
    if retryable and (scenario == "always-transient" or request_attempts[method] == 1):
        send(dict(id=request["id"], error={"code": -32801, "message": "content modified"}))
    elif method == "initialize":
        if scenario == "delayed":
            time.sleep(0.25)
        sync = {"openClose": True, "change": 1 if scenario == "full" else 2,
                "save": False if scenario == "false-save" else {"includeText": True}}
        capabilities = {"definitionProvider": True, "positionEncoding": encoding, "hoverProvider": True, "completionProvider": {"resolveProvider": True,"triggerCharacters": [".", ":"]},
                        "codeActionProvider": {"resolveProvider": True}, "renameProvider": True,
                        "referencesProvider": True, "documentFormattingProvider": True}
        if scenario != "none":
            capabilities["textDocumentSync"] = sync
        send(dict(id=request["id"], result={"capabilities": capabilities, "serverInfo":{"name":"Bedtime fixture","version":"1.0"}}))
        if scenario == "inherited":
            subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"])
    elif method == "textDocument/didOpen":
        documents[uri] = params["textDocument"]["text"]
        if scenario == "crash":
            sys.stderr.write("fixture startup failure\n")
            sys.stderr.flush()
            sys.exit(7)
        if scenario == "progress":
            for token in [7,"7"]:
                send(dict(id=1000 if token==7 else 1001, method="window/workDoneProgress/create",params={"token":token}))
                send(dict(method="$/progress",params={"token":token,"value":{"kind":"begin","title":"Indexing workspace","message":"src","percentage":10}}))
            for index in range(100):
                send(dict(method="window/logMessage",params={"type":3,"message":f"fixture log {index}"}))
            sys.stderr.write("fixture stderr ready\n")
            sys.stderr.flush()
        else:
          send(dict(method="textDocument/publishDiagnostics", params={
            "uri": uri, "version": params["textDocument"]["version"],
            "diagnostics": [{"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}},
                             "severity": 1, "message": "fixture diagnostic"}]}))
    elif method == "textDocument/didChange":
        for change in params["contentChanges"]:
            if "range" in change:
                text = documents[uri]
                start, end = (offset(text, change["range"][key]) for key in ("start", "end"))
                documents[uri] = text[:start] + change["text"] + text[end:]
            else:
                documents[uri] = change["text"]
        if scenario == "progress":
            for _ in range(300):
                send(dict(method="$/progress",params={"token":7,"value":{"kind":"report","percentage":50}}))
            send(dict(method="$/progress",params={"token":"7","value":{"kind":"end"}}))
        else:
            send(dict(method="textDocument/publishDiagnostics", params={"uri": uri,
                      "version": params["textDocument"]["version"], "diagnostics": []}))
    elif method == "textDocument/didSave" and scenario == "progress":
        send(dict(method="$/progress",params={"token":7,"value":{"kind":"end"}}))
    elif method == "textDocument/didClose":
        documents.pop(uri, None)
    elif scenario == "loading-empty" and method in ("textDocument/hover", "textDocument/definition") and request_attempts[method] == 1:
        send(dict(method="experimental/serverStatus",params={"health":"ok","quiescent":False}))
        send(dict(id=request["id"],result=None))
        send(dict(method="experimental/serverStatus",params={"health":"ok","quiescent":True}))
    elif method == "textDocument/definition":
        if scenario == "slow-definition":
            time.sleep(0.15)
        target = uri.rsplit("/", 1)[0] + "/target.rs" if scenario == "cross-file" else uri
        send(dict(id=request["id"], result={"uri": target, "range": {
            "start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}}))
    elif scenario == "cross-file" and method == "textDocument/hover" and request_attempts[method] == 1:
        send(dict(id=request["id"],result=None))
        send(dict(method="textDocument/publishDiagnostics",params={"uri":uri,"diagnostics":[]}))
    elif method == "textDocument/hover":
        if scenario == "slow-intelligence":
            time.sleep(0.15)
        send(dict(id=request["id"], result={"contents": {"kind":"markdown","value":"```rust\nfn first()\n```\nFixture **documentation**."}}))
    elif method == "textDocument/completion":
        if scenario == "slow-intelligence":
            time.sleep(0.15)
        row, col = params["position"]["line"], params["position"]["character"]
        send(dict(id=request["id"], result={"isIncomplete": True,"items":[
            {"label":"first", "sortText":"0","detail":"fn first(value)","insertText":"first(${1:value})$0","insertTextFormat":2,"data":{"fixture":1}},
            {"label":"second", "sortText":"1","insertText":"second"}]}))
    elif method == "completionItem/resolve":
        item = dict(params)
        item["documentation"] = {"kind":"markdown","value":"Resolved **completion documentation**."}
        item["additionalTextEdits"] = [{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":0}},"newText":"// auto import\n"}]
        send(dict(id=request["id"], result=item))
    elif method == "textDocument/codeAction":
        send(dict(id=request["id"], result=[
            {"title": "Add a comment", "kind": "quickfix", "data": {"uri": uri}},
            {"title": "Run the fixture command", "command": "fixture.edit", "arguments": [uri]},
            {"title": "Not here", "disabled": {"reason": "fixture says no"}}]))
    elif method == "codeAction/resolve":
        item = dict(params)
        item["edit"] = {"changes": {params["data"]["uri"]: [{"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}}, "newText": "// fixed\n"}]}}
        send(dict(id=request["id"], result=item))
    elif method == "workspace/executeCommand":
        target = params["arguments"][0]
        send(dict(id=5000, method="workspace/applyEdit", params={"edit": {"changes": {target: [
            {"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}}, "newText": "// command\n"}]}}}))
        send(dict(id=request["id"], result=None))
    elif method == "textDocument/rename":
        other = uri.rsplit("/", 1)[0] + "/target.rs"
        send(dict(id=request["id"], result={"changes": {
            uri: [{"range": {"start": {"line": 0, "character": 3}, "end": {"line": 0, "character": 7}}, "newText": params["newName"]}],
            other: [{"range": {"start": {"line": 0, "character": 3}, "end": {"line": 0, "character": 9}}, "newText": params["newName"]}]}}))
    elif method == "textDocument/references":
        other = uri.rsplit("/", 1)[0] + "/target.rs"
        send(dict(id=request["id"], result=[
            {"uri": uri, "range": {"start": {"line": 0, "character": 3}, "end": {"line": 0, "character": 7}}},
            {"uri": other, "range": {"start": {"line": 0, "character": 3}, "end": {"line": 0, "character": 9}}}]))
    elif method == "textDocument/formatting":
        send(dict(id=request["id"], result=[
            {"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}}, "newText": "// formatted\n"}]))
    elif method == "shutdown":
        send(dict(id=request["id"], result=None))
    record = dict(request, mirror=documents.get(uri), pid=os.getpid())
    with Path(transcript).open("a") as output:
        output.write(json.dumps(record, ensure_ascii=False) + "\n")
    if method == "exit":
        sys.exit(0)
    if method == "initialized" and scenario == "blocked":
        while True:
            time.sleep(1)
