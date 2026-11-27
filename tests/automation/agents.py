"""Agents: the panel lists the configured ACP clients; opening a client's
group starts its agent and lists its sessions; choosing one shows it in a
chat panel (once: choosing it again goes back to that panel); a new chat is
its own panel, and its thread is listed under the client by what was asked;
typing / lists the agent's commands and @ the workspace's files, over the
prompt, and Enter takes the one selected; the agent reads an open file as
its editor has it, and what it writes there is saved and shown; an unused
chat goes with its panel; and after a restart the open group's
agent is started and the chats are loaded again."""
import json
import sys
import tempfile
import time
from pathlib import Path
from bed import Bed

AGENT = Path(__file__).parent / "support" / "fake_acp_agent.py"


def until(bed, check, what, seconds=20):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if check():
            return
        bed.wait(10)
    bed.screenshot("agents_failed")
    raise AssertionError(f"never: {what}")


# The setting's value, as RON.
def quit(bed):
    """Cmd+Q, which saves on the way out; bed may be gone before it answers."""
    try:
        bed.key("Primary+Q")
    except (ConnectionError, OSError):
        pass
    bed.process.wait(20)


def row(bed, title):
    """The thread's row in the Agents panel (right of the chats' tabs), if
    it's listed."""
    agents = bed.find("Fake", role="Button")["rect"][0]
    rows = [w for w in bed.find_all(title, role="Button") if w["rect"][0] > agents]
    return rows[0] if rows else None


clients = f'[(name: "Fake", command: {json.dumps(sys.executable)}, args: [{json.dumps(str(AGENT))}])]'

with tempfile.TemporaryDirectory() as folder, tempfile.TemporaryDirectory() as config:
    (Path(folder) / "notes.txt").write_text("remember the milk\n")
    (Path(config) / "global").mkdir()
    (Path(config) / "global" / "settings.ron").write_text(
        '{\n    "appearance.cat": false,\n    "acp.clients": ' + clients + ',\n}\n')

    bed = Bed(folder, config=config)
    window = bed.status()["window"]
    bed.click((window["width"] * 0.6, window["height"] * 0.5))
    bed.wait(5)
    bed.invoke("agents.show")
    until(bed, lambda: bed.exists("Fake", role="Button"), "the client is listed")
    assert not bed.exists("Old session"), "a closed group lists nothing"
    bed.screenshot("agents_closed")

    # Opening the group starts the agent, which lists its sessions.
    bed.click("Fake", role="Button")
    until(bed, lambda: bed.exists("Old session"), "the agent's session is listed")
    bed.screenshot("agents_listed")

    # Choosing a thread shows it in a chat panel, loaded.
    bed.click("Old session")
    until(bed, lambda: bed.exists("Prompt"), "a chat panel opens")
    until(bed, lambda: bed.exists(contains="an earlier answer"), "the session is loaded")
    bed.screenshot("agents_loaded")

    # A new chat: asked, answered, and listed by what was asked.
    bed.click("New Fake chat", role="Button")
    until(bed, lambda: row(bed, "New chat"), "a new chat is listed")
    until(bed, lambda: bed.exists(contains="Start a conversation"), "its panel is shown")
    bed.click("Prompt")
    bed.wait(3)
    bed.type("hello there")
    bed.key("Return")
    until(bed, lambda: bed.exists(contains="echo: hello there"), "the agent answers")
    until(bed, lambda: row(bed, "hello there"), "the thread is named by the prompt")
    assert not row(bed, "New chat")
    bed.screenshot("agents_answered")

    # "/" lists the agent's commands over the prompt, narrowed by what's
    # typed; Enter puts the selected one in the prompt, and Enter again sends.
    bed.click("Prompt")
    bed.wait(3)
    bed.type("/pl")
    until(bed, lambda: bed.exists(contains="Commands: Enter"), "the commands are listed")
    assert bed.exists("/plan") and not bed.exists("/compact"), "narrowed to what's typed"
    bed.screenshot("agents_slash")
    bed.key("Return")
    until(bed, lambda: not bed.exists(contains="Commands: Enter"), "the list closes")
    assert not bed.exists(contains="echo: /pl"), "Enter chose the command, and sent nothing"
    bed.key("Return")
    until(bed, lambda: bed.exists(contains="echo: /plan"), "the command is sent")

    # "@" lists the workspace's files; Enter attaches the selected one.
    bed.type("see @not")
    until(bed, lambda: bed.exists(contains="Files: Enter"), "the files are listed")
    until(bed, lambda: bed.exists("notes.txt"), "the file is found")
    bed.screenshot("agents_files")
    bed.key("Return")
    until(bed, lambda: bed.exists("Remove attachment", role="Button"), "the file is attached")
    bed.screenshot("agents_attached")
    bed.key("Return")
    until(bed, lambda: bed.exists(contains="echo: see"), "the prompt is sent with it")
    assert bed.exists(contains="@notes.txt"), "what was sent names the file"
    bed.screenshot("agents_sent_file")

    # The agent reads and writes through bed. A file that's open is read as
    # its editor has it (with what was typed and not saved), and written
    # there: shown in the editor, and saved.
    notes = Path(folder) / "notes.txt"
    bed.drop_files([str(notes)], (window["width"] * 0.7, window["height"] * 0.5))
    until(bed, lambda: bed.exists(contains="remember the milk"), "the file opens")
    bed.click(bed.find(contains="remember the milk"))
    bed.wait(3)
    bed.type("UNSAVED")
    until(bed, lambda: bed.exists(contains="UNSAVED"), "the editor has what was typed")
    assert notes.read_text() == "remember the milk\n", "and it isn't saved"
    bed.click("Prompt")
    bed.wait(3)
    bed.type("read notes.txt")
    bed.key("Return")
    until(bed, lambda: bed.exists(contains="read 25 characters"), "the agent reads the editor's text")
    bed.type("write notes.txt")
    bed.key("Return")
    until(bed, lambda: bed.exists(contains="wrote it"), "the agent writes the file")
    until(bed, lambda: bed.exists(contains="written by the agent"), "the editor shows what it wrote")
    until(bed, lambda: notes.read_text() == "written by the agent\n", "and it's saved")
    bed.screenshot("agents_wrote")
    bed.click("Close notes.txt", role="Button")
    until(bed, lambda: bed.exists("Fake", role="Button"), "the Agents panel is in front again")

    # Choosing the first thread again goes back to its panel, not a new one.
    bed.click(row(bed, "Old session"))
    until(bed, lambda: bed.exists(contains="an earlier answer"), "the old chat is shown again")
    assert len(bed.find_all("Close Old session", visible=False)) == 1, "it has one panel"
    bed.screenshot("agents_back")

    # A chat nothing was said in goes with its panel.
    bed.click("New Fake chat", role="Button")
    until(bed, lambda: row(bed, "New chat"), "another new chat is listed")
    until(bed, lambda: bed.exists(contains="Start a conversation"), "its panel is shown")
    bed.click("Close New chat", role="Button")
    until(bed, lambda: not row(bed, "New chat"), "the unused chat is gone from the list")
    assert row(bed, "hello there") and row(bed, "Old session")
    quit(bed)
    assert bed.process.returncode == 0, bed.process.returncode

    # Started again: the open group starts its agent by itself, and the chat
    # panel that was showing has its session loaded again.
    with Bed(folder, config=config, build_first=False) as bed:
        until(bed, lambda: bed.exists("Fake", role="Button"), "the Agents panel is back")
        until(bed, lambda: row(bed, "Old session"), "the open group lists its sessions again")
        until(bed, lambda: bed.exists(contains="an earlier answer"), "the chat is loaded again")
        until(bed, lambda: row(bed, "hello there"), "the other chat's thread is listed again")
        bed.wait(10)
        assert not bed.exists("New chat"), "no chat was started in restoring"
        bed.screenshot("agents_restored")
