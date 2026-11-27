"""A stand-in ACP agent for tests/automation/agents.py: JSON-RPC over stdio,
one message a line. It lists one old session, replays it when loaded, has
one slash command (/plan), and answers a prompt with "echo: <the prompt>".
Asked to "read <file>" or "write <file>" (in the session's folder), it does
so through the client (fs/read_text_file, fs/write_text_file) and says how
it went."""
import json
import os
import sys

sessions = {"old-1": "Old session"}
folders = {}
calls = 0


def send(message):
    sys.stdout.write(json.dumps({"jsonrpc": "2.0", **message}) + "\n")
    sys.stdout.flush()


def say(session, kind, text):
    send({"method": "session/update", "params": {
        "sessionId": session,
        "update": {"sessionUpdate": kind, "content": {"type": "text", "text": text}},
    }})


def call(method, params):
    """Ask the client for something, and wait for its answer."""
    global calls
    calls += 1
    ident = f"call-{calls}"
    send({"id": ident, "method": method, "params": params})
    for line in sys.stdin:
        reply = json.loads(line) if line.strip() else {}
        if reply.get("id") == ident and "method" not in reply:
            if "error" in reply:
                raise RuntimeError(reply["error"].get("message"))
            return reply.get("result") or {}
    raise RuntimeError("the client went away")


def answer(session, text):
    """What's said back to a prompt."""
    verb, _, name = text.partition(" ")
    path = os.path.join(folders.get(session, "/"), name)
    try:
        if verb == "read":
            content = call("fs/read_text_file", {"sessionId": session, "path": path})["content"]
            return f"read {len(content)} characters"
        if verb == "write":
            call("fs/write_text_file",
                 {"sessionId": session, "path": path, "content": "written by the agent\n"})
            return "wrote it"
    except RuntimeError as error:
        return f"failed: {error}"
    return f"echo: {text}"


for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    request = json.loads(line)
    method, params, ident = request.get("method"), request.get("params") or {}, request.get("id")
    if ident is None:
        continue
    if method == "initialize":
        result = {
            "protocolVersion": 1,
            "agentCapabilities": {
                "loadSession": True,
                "sessionCapabilities": {"list": {}},
            },
            "agentInfo": {"name": "fake", "title": "Fake Agent", "version": "1.0"},
        }
    elif method == "session/new":
        name = f"new-{len(sessions)}"
        sessions[name] = name
        folders[name] = params.get("cwd") or "/"
        result = {"sessionId": name}
    elif method == "session/list":
        result = {"sessions": [
            {"sessionId": "old-1", "cwd": params.get("cwd") or "/", "title": "Old session"},
        ]}
    elif method == "session/load":
        say(params["sessionId"], "user_message_chunk", "an earlier question")
        say(params["sessionId"], "agent_message_chunk", "an earlier answer")
        result = {}
    elif method == "session/prompt":
        text = " ".join(block["text"] for block in params.get("prompt", []) if block.get("text"))
        say(params["sessionId"], "agent_message_chunk", answer(params["sessionId"], text))
        result = {"stopReason": "end_turn"}
    else:
        send({"id": ident, "error": {"code": -32601, "message": f"no {method}"}})
        continue
    send({"id": ident, "result": result})
    if method == "session/new":
        send({"method": "session/update", "params": {
            "sessionId": result["sessionId"],
            "update": {"sessionUpdate": "available_commands_update", "availableCommands": [
                {"name": "plan", "description": "Make a plan"},
                {"name": "compact", "description": "Shorten the conversation"},
            ]},
        }})
