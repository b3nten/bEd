# External automation

`bedtime.py` is a standard-library client for Bedtime applications. `bed.py` adds
bEd operations. Put this directory on `PYTHONPATH`, or run a script beside these
modules. Agents can create temporary scripts using the same API as the tests.

```python
from bed import Bed

with Bed.launch("target/debug/bed") as app:
    panel = app.open_file("/absolute/path/example.txt")
    app.wait_ready(panel)
    app.focus(panel)
    app.type_text("// inspected change\n")
    app.save(panel)
    assert not app.read_document(panel)["info"]["dirty"]
    app.capture(panel).save("target/editor.png")
    app.quit()
```

Launch helpers use a fresh configuration unless `config_dir` is supplied. They
create a private socket directory, name the client `smoke test`, and supply
`--allow-automation --approve-automation="smoke test"`. `quit()` requests an
orderly exit and checks the owned process; leaving the context closes the client
and terminates an owned process still running. Attaching never terminates the
application:

```python
from bed import Bed

with Bed.connect("/path/to/control.sock", name="layout inspection") as app:
    print(app.describe())
    print(app.state())
    app.capture().save("target/window.png")
```

## Permission and connection lifetime

External control is disabled by default. Enable it in Settings → Automation or
with `--allow-automation`. Each connection presents an Allow/Deny dialog before
it can inspect data, discover operations or control the application. Approval
expires on disconnect. Settings → Automation lists connections and permits
revocation. Applications without Settings still display manually approved
connections in a separate window.

`--approve-automation=NAME` requires `--allow-automation`. It may be repeated;
names match exactly and case-sensitively. Other names still require approval.
Both `=NAME` and a separate name argument are accepted. Launch overrides do not
change saved settings and are not propagated to new application windows.
Automatically approved clients do not display a floating status window, so they
do not obscure captures or intercept test input. Connection management remains
available in the Automation settings page.

Names are self-reported labels, not authenticated program identities. The
endpoint verifies the connecting OS user and resides in an owner-only directory.
Approved clients act with the application's permissions, including terminal
input. The transport provides no arbitrary ECS writes or GPU fault injection.
All remote input is blocked/cancelled while a connection awaits approval, and the
approval popup waits for held-input cleanup before opening.

A generated endpoint is reported on stderr as `AUTOMATION_ENDPOINT` followed by
JSON containing `socket`, `pid`, and `protocol`. `--automation-socket PATH`
selects an explicit path inside an existing private directory owned by the user;
an existing socket/file is never overwritten.

## Direct protocol

Use a persistent Unix stream socket with one JSON object per newline. IDs are
unsigned integers correlating requests and responses. Version 1 begins with:

```json
{"id":1,"method":"hello","params":{"protocol":1,"name":"smoke test"}}
```

The result identifies the application and connection, with `state` equal to
`approved` or `pending`. Pending clients can poll `connection_status`; other
methods are rejected until approval. Denial/revocation closes the stream.

Submit work, poll its receipt, and release it when no future capture needs it:

```json
{"id":2,"method":"submit","params":{"operation":"inspect","arguments":{}}}
{"id":3,"method":"poll","params":{"operation":1}}
{"id":4,"method":"release","params":{"operation":1}}
```

Submit returns an `operation` ID. Poll returns `pending`, or `complete` with the
completion `frame` and `value` or a structured `error`. A result payload is
consumed once; its small receipt remains until release. Python `call()` waits and
releases automatically. `submit()` gives an `Operation` context manager for
explicit dependencies:

```python
with app.submit("bedtime.settings.set", owner="bedtime.ui", key="fontSize", value=22) as change:
    change.wait()
    app.capture(panel, after=change).save("target/changed-font.png")
```

A capture waits for a subsequent UI frame after the prerequisite completes.
Failed dependencies fail the capture. Accepted captures pin their prerequisite
receipt until completion, even if the client releases it meanwhile. Receipt IDs
belong to one connection; panel/document handles include application and session
identity and must be treated as opaque strings.

Built-in operations are `describe`, `inspect`, `open_panel`, `focus_panel`,
`split_panel`, `close_panel`, `invoke`, `input`, `capture`, and, with document
support, `read_document`. `describe` lists extension names, descriptions and
argument/result JSON schemas. Call an extension with its name, for example
`app.call("bed.layout.select", layout=3)`.

Input arguments use `steps`, an array of arrays of events; each step occupies one
UI frame. Event types are `text`, `pointer`, `button`, `key`, `modifiers`, and
`scroll`. Key events use Winit physical key names (`KeyZ`, `Enter`) plus either
`character` or `named`; keys do not insert text. Python provides `type_text`,
`click`, and `shortcut` conveniences. Each sequence releases held input before
completion.

Captures return frame metadata and `images` containing `size`, `physical_rect`
and base64 `png`. Panel captures crop the final composited image: visible overlays
can cover their content. Hidden panels are not rendered on demand. Native
window decorations and the OS cursor are outside captures. Document byte
payloads use base64. Typed model applications expose their own structured
observations and commands rather than pretending models are text files.

The endpoint bounds connections, input/message sizes, operations, captures and
retained receipts. Release completed operations during long sessions. Disconnect
cancels unsubmitted work and held input; already submitted application operations
finish through their owners and their results are discarded, without rollback.

## Running coverage

```sh
cargo build --locked -p bed -p bedside-table
python3 -m unittest discover -s tools/automation/tests
cargo test-automation
cargo test-native
```

Use `python3 run_smokes.py --scenario bed` or `--scenario bedside` for a focused
scenario from this directory (supply binary/artifact paths), or invoke it from
the repo root as `python3 tools/automation/run_smokes.py`. Scripts run real
windows sequentially. Native integration tests require a desktop/GPU; the native
runner builds fixtures first. Add `--include-debugger` when Rust and LLDB are
available, or `--filter NAME` for focused native checks.

| Previous coverage | Current owner |
| --- | --- |
| bEd basic UI, document actions, shared views, appearance changes | Python `bed_scenario`; native window fixture preserves pixel assertions |
| bEd menu/layout identities, native chrome/material, terminal process/window preservation | `bed/tests/native` launching `bed-native-fixture` |
| Bedside seeded SQL input, shared undo/redo/Save, query results, initial/restart captures and restoration | Python `bedside_scenario` |
| Bedside cancellation, write rejection, dirty-query choices | Existing crate-local Rust tests |
| Renderer loss/recovery, canvas/icon retirement, input/capture recovery, window lifecycle | `bedtime/tests/native` launching existing independent fixtures |
| Workbench widgets, Git session routing, viewers and gameplay/model/editor composition invariants | `bedtime/tests/native`; existing fixtures retain their internal assertions |

The native fixtures use Rust access without enabling the external socket. Their
existing automation calls remain an in-process mechanism for exercising normal
application transitions; native hooks are not transport operations.
