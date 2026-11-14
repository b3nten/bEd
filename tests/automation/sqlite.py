"""SQLite: a database opens in the database viewer, listing its tables and
views; the one selected shows its rows (a page at a time, sorted and filtered
by SQLite) or its structure, and follows the file; Query runs SQL; cells
are changed and rows added and deleted in the grid."""
import sqlite3
import tempfile
from pathlib import Path
from bed import Bed


def until(bed, check, what, tries=300):
    for _ in range(tries):
        if check():
            return
        bed.wait(5)
    bed.screenshot("sqlite-failed")
    raise AssertionError(f"never: {what}")


def press(bed, target, check, what):
    """Click `target` (a label, or a function finding the widget) until
    `check` holds: now and then a click doesn't land."""
    for _ in range(4):
        bed.click(target() if callable(target) else target)
        for _ in range(60):
            if check():
                return
            bed.wait(5)
    bed.screenshot("sqlite-failed")
    raise AssertionError(f"never: {what}")


def row(bed, name):
    """The file tree's row (tabs and the panel's own rows are further right)."""
    return min(bed.find_all(name), key=lambda widget: widget["rect"][0])


def opened(bed, name):
    """Whether the file has a tab, besides its row in the tree."""
    return len(bed.find_all(name)) >= 2


def listed(bed, name):
    """The database panel's row for a table or view."""
    return max(bed.find_all(name), key=lambda widget: widget["rect"][0])


def run(bed, sql, check, what, key=False):
    """Type `sql` over what's in Query and run it, with the button or the key."""
    for _ in range(4):
        bed.click("SQL")
        bed.key("Primary+A")
        bed.type(sql)
        if key:
            bed.key("Primary+Enter")
        else:
            bed.click("Run")
        for _ in range(60):
            if check():
                return
            bed.wait(5)
    bed.screenshot("sqlite-failed")
    raise AssertionError(f"never: {what}")


def retype(bed, cell, text, check, what):
    """Select the cell showing `cell`, type `text` over it and keep it."""
    for _ in range(4):
        if bed.exists(cell):
            bed.click(listed(bed, cell))
            bed.type(text)
            bed.key("Enter")
        for _ in range(60):
            if check():
                return
            bed.wait(5)
    bed.screenshot("sqlite-failed")
    raise AssertionError(f"never: {what}")


def names(root):
    with sqlite3.connect(root / "music.db") as database:
        return [name for (name,) in database.execute("SELECT name FROM artist ORDER BY id")]


SCHEMA = """
CREATE TABLE artist (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE);
CREATE TABLE album (
    id INTEGER PRIMARY KEY,
    title VARCHAR(80) NOT NULL DEFAULT 'Untitled',
    artist_id INTEGER REFERENCES artist(id),
    year INTEGER
);
CREATE INDEX album_by_year ON album (year, title);
CREATE VIEW recent AS SELECT title FROM album WHERE year > 2000;
INSERT INTO artist (name) VALUES ('Low'), ('Grouper');
INSERT INTO album (title, artist_id, year) VALUES ('Things We Lost in the Fire', 1, 2001);
CREATE TABLE reading (id INTEGER PRIMARY KEY, sensor TEXT, level REAL, note TEXT, raw BLOB);
WITH RECURSIVE count(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM count LIMIT 2500)
INSERT INTO reading SELECT i, 'sensor ' || i, (i * 7919) % 2500 / 10.0, NULL, x'0102' FROM count;
"""

with tempfile.TemporaryDirectory() as folder:
    root = Path(folder)
    for name in ("music.db", "music.backup"):
        with sqlite3.connect(root / name) as database:
            database.executescript(SCHEMA)
    (root / "empty.db").write_bytes(b"")
    (root / "broken.db").write_text("This isn't a database, whatever its name says.\n")

    with Bed(folder, settings={"effects.enabled": True}) as bed:
        until(bed, lambda: bed.exists("music.db"), "the root is listed")

        # A database opens showing its first table's rows.
        # (The tree's rows are still settling: a click can land on another file.)
        bed.wait(30)
        press(bed, lambda: row(bed, "music.db"),
              lambda: opened(bed, "music.db") and bed.exists("Tables"),
              "music.db opens as a database")
        assert bed.exists("Views") and bed.exists("recent")
        until(bed, lambda: bed.exists("Things We Lost in the Fire"), "album's rows show")
        assert bed.exists("artist_id", role="Button") and bed.exists("1 row")
        bed.screenshot("sqlite-data")

        # Its structure.
        press(bed, "Structure", lambda: bed.exists("not null, default 'Untitled'"),
              "album's columns show")
        assert bed.exists("references artist(id)")
        assert bed.exists("album_by_year") and bed.exists("year, title")
        assert bed.exists("Table, 4 columns")
        bed.screenshot("sqlite-structure")

        # Another table, and a view.
        press(bed, lambda: listed(bed, "artist"), lambda: bed.exists("Table, 2 columns"),
              "artist shows")
        assert bed.exists("unique, from a constraint")
        press(bed, lambda: listed(bed, "recent"), lambda: bed.exists("View, 1 column"),
              "the view shows")

        # A change made by another program shows up.
        with sqlite3.connect(root / "music.db") as database:
            database.executescript("CREATE TABLE label (id INTEGER PRIMARY KEY, name TEXT);")
        until(bed, lambda: bed.exists("label"), "the new table is listed")
        assert bed.exists("View, 1 column"), "the selection stays"
        assert not bed.status()["errors"], bed.status()["errors"]

        # A large table comes a page at a time.
        press(bed, lambda: listed(bed, "reading"), lambda: bed.exists("Table, 5 columns"),
              "reading is selected")
        press(bed, "Data", lambda: bed.exists("Rows 1–1,000 of 2,500"), "the first page shows")
        assert bed.exists("Page 1 of 3") and bed.exists("sensor 1")
        press(bed, "chevron-right", lambda: bed.exists("Rows 1,001–2,000 of 2,500"),
              "the second page shows")
        assert bed.exists("sensor 1001") and bed.exists("Page 2 of 3")
        press(bed, "chevron-left", lambda: bed.exists("Page 1 of 3"), "back to the first page")

        # Clicking a column's name sorts by it: SQLite sorts every row, not the page.
        level = lambda: bed.find("level", role="Button")
        press(bed, level, lambda: bed.exists("sensor 2500"), "sorted by level")
        press(bed, level, lambda: bed.exists("sensor 2321"), "sorted by level, descending")
        press(bed, level, lambda: bed.exists("sensor 1"), "not sorted")

        # The filter is an SQL condition.
        bed.click("Filter rows")
        bed.type("id % 500 = 0")
        bed.key("Enter")
        until(bed, lambda: bed.exists("5 rows"), "the filter applies")
        assert bed.exists("sensor 500") and not bed.exists("sensor 1")
        bed.screenshot("sqlite-filter")
        bed.click("Filter rows")
        bed.type(" and")
        bed.key("Enter")
        until(bed, lambda: bed.exists(contains="These rows can't be read"), "a bad filter says why")

        # Another program's rows show up, where the grid already is.
        press(bed, lambda: listed(bed, "artist"), lambda: bed.exists("Grouper"),
              "artist's rows show")
        with sqlite3.connect(root / "music.db") as database:
            database.execute("INSERT INTO artist (name) VALUES ('Stars of the Lid')")
        until(bed, lambda: bed.exists("Stars of the Lid"), "the new row shows")
        assert bed.exists("3 rows")
        assert not bed.status()["errors"], bed.status()["errors"]

        # Query runs what's typed, with Cmd/Ctrl+Enter, and shows the rows.
        press(bed, "Query", lambda: bed.exists("Run"), "Query shows")
        run(bed, "select name from artist order by name",
            lambda: bed.exists(contains="3 rows ·"), "the query runs", key=True)
        assert bed.exists("Grouper") and bed.exists("name", role="Button")
        bed.screenshot("sqlite-query")

        # Several statements; ones that write are kept, and Data follows.
        run(bed, "insert or ignore into artist (name) values ('Earth'); select count(*) as n from artist",
            lambda: bed.exists(contains="2 statements · 1 row · "), "both statements run")
        with sqlite3.connect(root / "music.db") as database:
            assert database.execute("SELECT count(*) FROM artist WHERE name = 'Earth'").fetchone() == (1,)
        press(bed, "Data", lambda: bed.exists("Earth") and bed.exists("4 rows"),
              "Data shows the new row")

        # A mistake says what SQLite says.
        press(bed, "Query", lambda: bed.exists("Run"), "Query shows again")
        run(bed, "selec 1", lambda: bed.exists(contains="syntax error"), "the mistake is reported")

        # One that would never finish can be stopped.
        run(bed, "with recursive c(i) as (select 1 union all select i + 1 from c) select max(i) from c",
            lambda: bed.exists("Cancel"), "the endless query is running")
        press(bed, "Cancel", lambda: bed.exists("Interrupted"), "it stops")
        assert bed.exists("Run")
        assert not bed.status()["errors"], bed.status()["errors"]

        # A cell is changed by typing over it; the file has it at once.
        press(bed, "Data", lambda: bed.exists("Earth") and bed.exists("Add Row"), "back to Data")
        retype(bed, "Grouper", "Liz Harris", lambda: "Liz Harris" in names(root),
               "the cell is written to the file")
        until(bed, lambda: bed.exists("Liz Harris") and not bed.exists("Grouper"),
              "the grid shows the change")

        # What SQLite refuses is said under the rows, and nothing changes.
        retype(bed, "Low", "Earth", lambda: bed.exists(contains="UNIQUE constraint failed"),
               "a duplicate name is refused")
        assert names(root) == ["Low", "Liz Harris", "Stars of the Lid", "Earth"]

        # A new row is filled in, then inserted.
        press(bed, "Add Row", lambda: bed.exists("Insert Row"), "a row is being made")
        bed.screenshot("sqlite-new-row")
        press(bed, "Insert Row", lambda: bed.exists(contains="NOT NULL constraint failed"),
              "a row without a name is refused")
        assert bed.exists("Insert Row"), "the row being made stays"
        bed.key("Right")
        bed.type("Sunn O)))")
        bed.key("Enter")
        press(bed, "Insert Row", lambda: bed.exists("5 rows"), "the row is inserted")
        assert names(root)[-1] == "Sunn O)))", names(root)
        assert bed.exists("Add Row") and not bed.exists(contains="constraint failed")
        press(bed, "Add Row", lambda: bed.exists("Discard"), "another row is being made")
        press(bed, "Discard", lambda: bed.exists("Add Row"), "and is given up")

        # Deleting a row asks first.
        def delete_row():
            bed.click(listed(bed, "Earth"), button="Right")
            bed.wait(10)
            return bed.find("Delete Row")
        press(bed, delete_row, lambda: bed.exists("Delete row 4?"), "deleting asks")
        press(bed, "Keep", lambda: bed.exists("Add Row"), "the row is kept")
        assert "Earth" in names(root)
        press(bed, delete_row, lambda: bed.exists("Delete row 4?"), "deleting asks again")
        press(bed, "Delete", lambda: bed.exists("4 rows"), "the row is deleted")
        assert names(root) == ["Low", "Liz Harris", "Stars of the Lid", "Sunn O)))"]
        bed.screenshot("sqlite-edited")

        # A view's rows can't be changed.
        press(bed, lambda: listed(bed, "recent"), lambda: bed.exists("1 row"), "the view's rows show")
        assert not bed.exists("Add Row")
        assert not bed.status()["errors"], bed.status()["errors"]

        # An empty database: Query makes its first table.
        press(bed, lambda: row(bed, "empty.db"),
              lambda: opened(bed, "empty.db") and bed.exists("No tables"), "empty.db opens")
        press(bed, "Query", lambda: bed.exists("Run"), "Query shows in the empty database")
        run(bed, "create table if not exists first (a, b)", lambda: bed.exists("first"),
            "the new table is listed")
        press(bed, "Structure", lambda: bed.exists("Table, 2 columns"), "and shows its structure")

        # A database is known by how it starts, not only by its name.
        press(bed, lambda: row(bed, "music.backup"),
              lambda: opened(bed, "music.backup") and bed.exists("Things We Lost in the Fire"),
              "music.backup opens as a database")

        # A file that only has the name says so.
        press(bed, lambda: row(bed, "broken.db"),
              lambda: bed.exists(contains="This database can't be read"), "broken.db says why")
        bed.screenshot("sqlite-broken")
        assert not bed.status()["errors"], bed.status()["errors"]
