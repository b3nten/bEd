"""Retarget the Documents/cats Rigify library onto the repaired toon cat.

Run with Blender, which provides bpy and mathutils:
  Blender --background --factory-startup --disable-autoexec --python assets/cat/retarget.py -- \
    --source '~/Documents/cats/Cat Black.blend' --output assets/cat/cat.retarget.blend --export-glb

Default: Cat_Walk (1_32). Use --list to list clips, --action 'exact name' for
another clip, or --all to bake every multi-frame clip into one .blend file.
Select a baked action on Cat_Rig in Blender's Action Editor to inspect it.

This mapping is specific to these two cat rigs. It samples the evaluated Rigify
skeleton (including IK/drivers), transfers rest-relative rotations, preserves
the toon skeleton's lengths, and solves legs to proportion-adjusted paw paths.
Facial detail has no equivalent on the toon rig; eyes/mouth follow the head.
Inputs are read only. Output paths must differ from both input files.
"""

import argparse
import json
import math
import sys
from pathlib import Path

import bpy
from mathutils import Matrix, Quaternion, Vector


ROTATION_MAP = {
    'torso_02': 'ORG-spine',
    'spine.01_012': 'ORG-spine.002',
    'spine.02_013': 'ORG-spine.003',
    'neck_017': 'ORG-spine.005',
    'head_018': 'ORG-spine.006',
    'tail_07': 'ORG-tail.001',
    'tail.01_08': 'ORG-tail.002',
    'tail.02_09': 'ORG-tail.003',
    'tail.03_010': 'ORG-tail.004',
    'tail.end_011': 'ORG-tail.006',
    'thigh.B.L_03': 'ORG-thigh.L',
    'leg.upper.B.L_04': 'ORG-shin.L',
    'leg.lower.B.L_05': 'ORG-foot.L',
    'foot.B.L_06': 'ORG-r_toe.L',
    'thigh.B.R_026': 'ORG-thigh.R',
    'leg.upper.B.R_027': 'ORG-shin.R',
    'leg.lower.B.R_028': 'ORG-foot.R',
    'foot.B.R_029': 'ORG-r_toe.R',
    'leg.upper.F.L_014': 'ORG-upper_arm.L',
    'leg.lower.FL_015': 'ORG-forearm.L',
    'foot.F.L_016': 'ORG-f_toe.L',
    'leg.upper.F.R_024': 'ORG-upper_arm.R',
    'leg.lower.F.R_00': 'ORG-forearm.R',
    'foot.F.R_025': 'ORG-f_toe.R',
}
LEGS = (
    ('front.L', ('leg.upper.F.L_014', 'leg.lower.FL_015'), 'foot.F.L_016',
     ('ORG-upper_arm.L', 'ORG-forearm.L'), 'ORG-f_toe.L'),
    ('front.R', ('leg.upper.F.R_024', 'leg.lower.F.R_00'), 'foot.F.R_025',
     ('ORG-upper_arm.R', 'ORG-forearm.R'), 'ORG-f_toe.R'),
    ('back.L', ('thigh.B.L_03', 'leg.upper.B.L_04', 'leg.lower.B.L_05'), 'foot.B.L_06',
     ('ORG-thigh.L', 'ORG-shin.L', 'ORG-foot.L'), 'ORG-r_toe.L'),
    ('back.R', ('thigh.B.R_026', 'leg.upper.B.R_027', 'leg.lower.B.R_028'), 'foot.B.R_029',
     ('ORG-thigh.R', 'ORG-shin.R', 'ORG-foot.R'), 'ORG-r_toe.R'),
)


def solve_leg(points, lengths, goal):
    """FABRIK: move joints to the goal, with fixed root and segment lengths."""
    points = [p.copy() for p in points]
    root = points[0].copy()
    reach = sum(lengths)
    if (goal-root).length >= reach:
        direction = (goal-root).normalized()
        for i, length in enumerate(lengths):
            points[i+1] = points[i] + direction * length
    else:
        for _ in range(48):
            points[-1] = goal.copy()
            for i in range(len(lengths)-1, -1, -1):
                points[i] = points[i+1] + (points[i]-points[i+1]).normalized()*lengths[i]
            points[0] = root.copy()
            for i, length in enumerate(lengths):
                points[i+1] = points[i] + (points[i+1]-points[i]).normalized()*length
            if (points[-1]-goal).length < 1e-6:
                break
    return points


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--source', default='~/Documents/cats/Cat Black.blend')
    parser.add_argument('--target', default=str(Path(__file__).with_name('cat.blend')))
    parser.add_argument('--output', default=str(Path(__file__).with_name('cat.retarget.blend')))
    selection = parser.add_mutually_exclusive_group()
    selection.add_argument('--action')
    selection.add_argument('--all', action='store_true')
    parser.add_argument('--list', action='store_true')
    parser.add_argument('--step', type=float, default=0.5, help='Source-frame sampling interval (default 0.5)')
    parser.add_argument('--fps', type=int, default=24, help='Source library frame rate, verified as 24 fps')
    parser.add_argument('--export-glb', action='store_true',
                        help='Export a GLB beside the blend; --all includes all baked clips and the original Walk')
    args = parser.parse_args(sys.argv[sys.argv.index('--')+1:] if '--' in sys.argv else [])
    source_path = Path(args.source).expanduser().resolve()
    target_path = Path(args.target).expanduser().resolve()
    output_path = Path(args.output).expanduser().resolve()
    if not source_path.is_file() or not target_path.is_file():
        parser.error('Both source and target Blender files must exist')
    if output_path in (source_path, target_path):
        parser.error('Choose a separate output file; input models are read only')
    if args.step <= 0 or args.fps <= 0:
        parser.error('--step and --fps must be positive')

    bpy.ops.wm.open_mainfile(filepath=str(target_path), load_ui=False, use_scripts=False)
    scene = bpy.context.scene
    target = bpy.data.objects['Cat_Rig']
    mesh = bpy.data.objects['Cat']
    original_objects = set(bpy.data.objects)
    original_actions = set(bpy.data.actions)
    original_walk = target.animation_data.action
    original_walk.use_fake_user = True
    with bpy.data.libraries.load(str(source_path), link=False) as (data_from, data_to):
        if 'cat' not in data_from.objects:
            parser.error('Expected the source Rigify armature named cat')
        data_to.objects = ['cat']
    source = data_to.objects[0]
    source_collection = bpy.data.collections.new('Retarget source (temporary)')
    scene.collection.children.link(source_collection)
    source_collection.objects.link(source)
    source.hide_viewport = False
    source.hide_set(False)
    source.data.pose_position = 'POSE'
    source.animation_data.use_nla = False
    source_actions = sorted({s.action for t in source.animation_data.nla_tracks
                             for s in t.strips if s.action}, key=lambda a: a.name)
    clips = [a for a in source_actions if a.frame_range[1] > a.frame_range[0]]
    if args.list:
        for action in source_actions:
            print(f'{action.name!r}: frames {action.frame_range[0]:g}–{action.frame_range[1]:g}', flush=True)
        return
    chosen_name = args.action or 'Cat_Walk (1_32)'
    chosen = clips if args.all else [a for a in source_actions if a.name == chosen_name]
    if not chosen:
        parser.error(f'No action named {chosen_name!r}; use --list')

    source_rest = {b.name: source.matrix_world @ b.matrix_local for b in source.data.bones}
    needed = set(ROTATION_MAP.values()) | {leg[-1] for leg in LEGS}
    if needed - source_rest.keys() or ROTATION_MAP.keys() - target.data.bones.keys():
        parser.error('The input skeletons do not match the documented bone map')
    target_rest = {b.name: b.matrix_local.copy() for b in target.data.bones}
    target_quats = {name: matrix.to_quaternion() for name, matrix in target_rest.items()}
    source_quats = {name: matrix.to_quaternion() for name, matrix in source_rest.items()}
    parents = {b.name: b.parent.name if b.parent else None for b in target.data.bones}
    order = []
    def visit(bone):
        order.append(bone.name)
        for child in bone.children:
            visit(child)
    for bone in target.data.bones:
        if bone.parent is None:
            visit(bone)
    body_bones = set()
    for name in order:
        if name == 'torso_02' or parents[name] in body_bones:
            body_bones.add(name)
    target_body = target_rest['torso_02'].translation
    source_body = source_rest['ORG-spine'].translation
    body_scale = ((target_rest['spine.02_013'].translation-target_body).length /
                  (source_rest['ORG-spine.003'].translation-source_body).length)
    paw_data = {}
    for label, chain, paw, source_chain, toe in LEGS:
        group = mesh.vertex_groups[paw].index
        vertices = [v.co.copy() for v in mesh.data.vertices
                    if any(g.group == group and g.weight > 0.5 for g in v.groups)]
        if not vertices:
            raise ValueError(f'No paw geometry weighted to {paw}')
        minimum = min(v.z for v in vertices)
        sole_points = [v for v in vertices if v.z < minimum + 0.025]
        sole = sum(sole_points, Vector()) / len(sole_points)
        sole.z = minimum
        paw_data[label] = {
            'sole': sole,
            'local_sole': target_rest[paw].inverted() @ sole,
            'local_vertices': [target_rest[paw].inverted() @ v for v in vertices],
            'source_sole': source.matrix_world @ source.data.bones[toe].tail_local,
            'scale': sum(target.data.bones[n].length for n in chain) /
                     sum(source.data.bones[n].length for n in source_chain),
        }
    source_properties = {b.name: {k: v for k, v in b.items() if isinstance(v, (int, float, bool))}
                         for b in source.pose.bones}
    report = {'source': source_path.name, 'target': target_path.name, 'fps': args.fps,
              'rotation_map': ROTATION_MAP, 'body_motion_scale': body_scale, 'actions': []}
    scene.render.fps = args.fps
    scene.render.fps_base = 1
    new_actions = []

    for source_action in chosen:
        # Reset once per clip: these source actions key different subsets of the
        # controls, and must never inherit the preceding clip's cached pose.
        source.animation_data.action = None
        for bone in source.pose.bones:
            bone.matrix_basis = Matrix.Identity(4)
            for key, value in source_properties[bone.name].items():
                bone[key] = value
        source.animation_data.action = source_action
        if source_action.slots:
            source.animation_data.action_slot = source_action.slots[0]
        target.animation_data.action = None
        for bone in target.pose.bones:
            bone.rotation_mode = 'QUATERNION'
            bone.matrix_basis = Matrix.Identity(4)
        baked = bpy.data.actions.new('Retarget / ' + source_action.name.strip())
        baked.use_fake_user = True
        target.animation_data.action = baked
        start, end = map(float, source_action.frame_range)
        sample_times = [min(start+i*args.step, end)
                        for i in range(math.ceil((end-start)/args.step)+1)]
        previous_quaternions = {}
        maximum_reach_error = 0.0
        height_corrections = []
        ground_ranges = {label: [] for label, *_ in LEGS}
        loop_positions = []
        for time in sample_times:
            scene.frame_set(math.floor(time), subframe=time % 1)
            evaluated = source.evaluated_get(bpy.context.evaluated_depsgraph_get())
            posed = {name: source.matrix_world @ evaluated.pose.bones[name].matrix for name in needed}
            body_rotation = posed['ORG-spine'].to_quaternion() @ source_quats['ORG-spine'].inverted()
            body_position = target_body + (posed['ORG-spine'].translation-source_body)*body_scale
            desired = {}
            rotations = {}
            for name in order:
                parent = parents[name]
                if name in ROTATION_MAP:
                    source_name = ROTATION_MAP[name]
                    rotation = (posed[source_name].to_quaternion() @ source_quats[source_name].inverted()
                                @ target_quats[name])
                elif parent:
                    rotation = rotations[parent] @ target_quats[parent].inverted() @ target_quats[name]
                else:
                    rotation = target_quats[name].copy()
                position = (desired[parent] @ target_rest[parent].inverted() @ target_rest[name].translation
                            if parent else target_rest[name].translation.copy())
                if name == 'torso_02':
                    position = body_position
                rotations[name] = rotation
                desired[name] = Matrix.LocRotScale(position, rotation, Vector((1, 1, 1)))

            paw_goals = {}
            height_correction = 0.0
            for label, chain, paw, source_chain, toe in LEGS:
                data = paw_data[label]
                source_sole = source.matrix_world @ evaluated.pose.bones[toe].tail
                local_change = (body_rotation.inverted() @ (source_sole-posed['ORG-spine'].translation)
                                - (data['source_sole']-source_body))
                sole_goal = body_position + body_rotation @ (
                    data['sole']-target_body + local_change*data['scale'])
                # The source's global sole height preserves planted paws even
                # though the body and the two leg types use different scales.
                contact_height = data['sole'].z + (source_sole.z-data['source_sole'].z)*data['scale']
                rotation = rotations[paw]
                local_minimum = min((rotation @ p).z for p in data['local_vertices'])
                goal = sole_goal - rotation @ data['local_sole']
                goal.z = contact_height - local_minimum
                paw_goals[label] = (goal, local_minimum)
                # Lower the body when a planted paw would otherwise force a
                # short toon leg to stretch. The small reach margin keeps the
                # joints away from a completely straight, unstable pose.
                delta = desired[chain[0]].translation-goal
                reach = sum(target.data.bones[n].length for n in chain)*0.99
                if delta.z > 0 and delta.x*delta.x+delta.y*delta.y < reach*reach:
                    maximum_height = math.sqrt(reach*reach-delta.x*delta.x-delta.y*delta.y)
                    height_correction = max(height_correction, delta.z-maximum_height)
            height_corrections.append(height_correction)
            for name in body_bones:
                desired[name].translation.z -= height_correction

            for label, chain, paw, source_chain, toe in LEGS:
                goal, local_minimum = paw_goals[label]
                points = [desired[n].translation for n in chain] + [desired[paw].translation]
                lengths = [target.data.bones[n].length for n in chain]
                solved = solve_leg(points, lengths, goal)
                reach_error = (solved[-1]-goal).length
                maximum_reach_error = max(maximum_reach_error, reach_error)
                for i, name in enumerate(chain):
                    direction = (solved[i+1]-solved[i]).normalized()
                    rotation = (rotations[name] @ Vector((0, 1, 0))).rotation_difference(direction) @ rotations[name]
                    desired[name] = Matrix.LocRotScale(solved[i], rotation, Vector((1, 1, 1)))
                desired[paw] = Matrix.LocRotScale(solved[-1], rotations[paw], Vector((1, 1, 1)))
                ground_ranges[label].append(solved[-1].z + local_minimum)

            # Bake only onto the toon rig; the imported source is removed below.
            output_frame = 1 + time-start
            for name in order:
                bone = target.pose.bones[name]
                parent = parents[name]
                kwargs = {'parent_matrix': desired[parent], 'parent_matrix_local': target_rest[parent]} if parent else {}
                basis = bone.bone.convert_local_to_pose(desired[name], target_rest[name], invert=True, **kwargs)
                location, rotation, scale = basis.decompose()
                if max(abs(v-1) for v in scale) > 0.0001:
                    raise ValueError(f'Unexpected bone scaling: {name}: {tuple(scale)}')
                previous = previous_quaternions.get(name)
                if previous and rotation.dot(previous) < 0:
                    rotation.negate()
                previous_quaternions[name] = rotation.copy()
                bone.location = location
                bone.rotation_quaternion = rotation
                bone.scale = (1, 1, 1)
                bone.keyframe_insert('location', frame=output_frame, group=name)
                bone.keyframe_insert('rotation_quaternion', frame=output_frame, group=name)
            if time in (sample_times[0], sample_times[-1]):
                loop_positions.append({name: desired[name].copy() for name in order})
        for layer in baked.layers:
            for strip in layer.strips:
                for bag in strip.channelbags:
                    for curve in bag.fcurves:
                        for key in curve.keyframe_points:
                            key.interpolation = 'LINEAR'
        invalid = [d.data_path for d in source.animation_data.drivers if not d.is_valid]
        if invalid:
            raise ValueError(f'Source drivers could not evaluate: {invalid}')
        baked['source_file'] = source_path.name
        baked['source_action'] = source_action.name
        baked['retarget_method'] = 'rest-relative rotations and proportion-adjusted paw IK'
        result = {'source_action': source_action.name, 'baked_action': baked.name,
                  'frames': [1, 1+end-start], 'samples': len(sample_times),
                  'max_paw_reach_error': maximum_reach_error,
                  'body_height_correction': [min(height_corrections), max(height_corrections)],
                  'paw_ground_ranges': {k: [min(v), max(v)] for k, v in ground_ranges.items()},
                  'loop_position_difference': max((loop_positions[0][n].translation-loop_positions[-1][n].translation).length
                                                  for n in order) if len(loop_positions) == 2 else 0}
        report['actions'].append(result)
        new_actions.append(baked)
        print('RETARGETED', json.dumps(result), flush=True)

    # Remove every source datablock before saving: review files contain the
    # original toon mesh/rig plus baked actions, with no Rigify dependency.
    source.animation_data_clear()
    for obj in list(bpy.data.objects):
        if obj not in original_objects:
            bpy.data.objects.remove(obj, do_unlink=True)
    bpy.data.collections.remove(source_collection)
    for action in list(bpy.data.actions):
        if action not in original_actions and action not in new_actions:
            bpy.data.actions.remove(action)
    bpy.data.orphans_purge(do_local_ids=True, do_linked_ids=False, do_recursive=True)
    target.animation_data.action = new_actions[0]
    if new_actions[0].slots:
        target.animation_data.action_slot = new_actions[0].slots[0]
    scene.frame_start = 1
    scene.frame_end = math.ceil(new_actions[0].frame_range[1])
    scene.frame_set(1)
    bpy.ops.object.select_all(action='DESELECT')
    target.select_set(True)
    bpy.context.view_layer.objects.active = target
    evaluated_mesh = mesh.evaluated_get(bpy.context.evaluated_depsgraph_get())
    bounds = [evaluated_mesh.matrix_world @ Vector(c) for c in evaluated_mesh.bound_box]
    low = Vector([min(p[i] for p in bounds) for i in range(3)])
    high = Vector([max(p[i] for p in bounds) for i in range(3)])
    # Open in a useful material view, with the animation-owning rig selected.
    for screen in bpy.data.screens:
        for area in screen.areas:
            if area.type == 'VIEW_3D':
                space = area.spaces.active
                space.overlay.show_overlays = False
                space.shading.type = 'MATERIAL'
                space.shading.use_scene_world = False
                space.shading.use_scene_lights = False
                space.region_3d.view_location = (low+high)/2
                space.region_3d.view_rotation = Vector((-0.9, 1.5, -0.6)).to_track_quat('-Z', 'Y')
                space.region_3d.view_distance = max(high-low)*1.6
    output_path.parent.mkdir(parents=True, exist_ok=True)
    report_path = output_path.with_suffix('.json')
    report_path.write_text(json.dumps(report, indent=2)+'\n')
    if args.export_glb:
        mesh.select_set(True)
        bpy.ops.export_scene.gltf(filepath=str(output_path.with_suffix('.glb')), export_format='GLB',
                                  use_selection=True, export_animations=True,
                                  export_animation_mode='ACTIONS' if args.all else 'ACTIVE_ACTIONS',
                                  export_frame_range=not args.all,
                                  export_skins=True, export_extras=True)
        mesh.select_set(False)
    bpy.context.preferences.filepaths.save_version = 0
    bpy.ops.wm.save_as_mainfile(filepath=str(output_path), compress=True)
    print('SAVED', output_path, 'REPORT', report_path, flush=True)


if __name__ == '__main__':
    main()
