"""Write the cat as bedtime-cat reads it: `bedtime/crates/bedtime-cat/assets/cat.glb`.

    /Applications/Blender.app/Contents/MacOS/Blender -b bedtime/crates/bedtime-cat/assets/cat.blend --python tools/cat/export.py

Run this after changing `cat.blend`. Every action becomes an
animation of the same name, sampled each frame on the bones that carry skin:
the controls and what they do to the legs are baked in. bEd finds bones and
animations by name (`bed/visuals/cat/rig.rs`): add a move there to use a
new action.
"""

import os

import bpy


def export(path):
    arm = bpy.data.objects["cat_rig"]
    # The exporter writes the actions it finds on the armature: each is
    # stashed there, as Blender's own "stash" does.
    data = arm.animation_data_create()
    for track in list(data.nla_tracks):
        data.nla_tracks.remove(track)
    for action in sorted(bpy.data.actions, key=lambda action: action.name):
        track = data.nla_tracks.new()
        track.name = action.name
        track.mute = True
        strip = track.strips.new(action.name, int(action.frame_range[0]), action)
        strip.name = action.name
    bpy.ops.export_scene.gltf(
        filepath=path,
        export_format="GLB",
        export_animations=True,
        export_animation_mode="ACTIONS",
        export_force_sampling=True,
        export_optimize_animation_size=False,
        export_def_bones=True,
        export_skins=True,
        export_influence_nb=4,
        export_yup=True,
        export_cameras=False,
        export_lights=False,
    )
    print(f"Wrote {path}: {os.path.getsize(path) // 1024} KiB")


if __name__ == "__main__":
    here = os.path.dirname(os.path.abspath(__file__))
    export(os.path.normpath(os.path.join(here, "..", "..", "bedtime", "crates", "bedtime-cat", "assets", "cat.glb")))
