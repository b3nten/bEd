"""bEd conveniences over the shared Bedtime transport and registered operations."""
import os
from bedtime import Client, Panel


class Bed(Client):
    @classmethod
    def launch(cls, binary="target/debug/bed", *, args=(), **options):
        return super().launch(binary, args=("--new-window", *args), **options)

    def open_file(self, path, *, viewer=None, additional=False, move_existing=False, source=None):
        value = self.call("bed.open_file", path=os.fspath(path), viewer=viewer,
                          additional=additional, move_existing=move_existing,
                          source=self._handle(source))
        return Panel(value["panel"], value.get("document"))

    def select_layout(self, layout):
        return self.call("bed.layout.select", layout=layout)

    def reset_layout(self):
        return self.call("bed.layout.reset")

    def open_workspace(self, path):
        return self.call("bed.workspace.open", path=os.fspath(path))

    def save_workspace(self, name=None):
        return self.call("bed.workspace.save", name=name)

    def state(self):
        return self.call("bed.state")
