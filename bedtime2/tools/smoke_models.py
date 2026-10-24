#!/usr/bin/env python3
"""Native model viewing, controls, sidecars, isolation, source edits and restoration."""
import argparse
import base64
import json
import os
from pathlib import Path
import struct
import tempfile
import zlib

from automation import Application
from smoke_bed2 import center, open_panel, ready, workspace
from smoke_editor import primary
from smoke_filetree import inspection, wait_value
from smoke_images import choose, current
from smoke_workspace_policy import clean_quit


def png(color):
    def chunk(kind, data):
        return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind + data))
    return b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', 1, 1, 8, 6, 0, 0, 0)) + chunk(b'IDAT', zlib.compress(b'\0' + bytes(color))) + chunk(b'IEND', b'')


def model(app, panel):
    value = current(app, panel)
    assert value['error'] is None, value
    return value['model']


def loaded(app, panel):
    wait_value(app, lambda s: any(p['entity'] == panel and p['model'] and p['model']['ready'] and not p['loading'] and not p['model']['loading'] and p['model']['error'] is None for p in s['previews']))
    app.wait_until(lambda s: s['render']['pending_pipelines'] == 0)
    app.request('wait_frames', frames=20)
    return model(app, panel)


def capture(app, name, panel=None):
    if panel:
        loaded(app, panel)
    result = app.capture(name, **({'viewport': panel} if panel else {}))
    assert result['nontransparent_pixels'] > 100, (name, result)
    return result


def open_model(app, layout, path):
    panel = app.request('app.open_file', layout=layout, path=str(path))['panel']
    loaded(app, panel)
    return panel


def escape(app):
    app.input({'type':'key','code':'Escape','pressed':True}, {'type':'key','code':'Escape','pressed':False})


def select(app, combo, value):
    choose(app, 'Appearance')
    choose(app, combo)
    choose(app, value)
    escape(app)
    app.request('wait_frames', frames=20)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', default='target/debug/bed2')
    parser.add_argument('--output', default='target/models-smoke')
    args = parser.parse_args()
    output = Path(args.output).resolve()
    output.mkdir(parents=True, exist_ok=True)
    previous = os.environ.get('XDG_CONFIG_HOME')
    captures = []
    try:
        with tempfile.TemporaryDirectory(prefix='fixture-', dir=output) as temporary:
            root = Path(temporary) / 'project'
            (root / 'models').mkdir(parents=True)
            (root / 'textures').mkdir()
            os.environ['XDG_CONFIG_HOME'] = str(Path(temporary) / 'config')
            source = json.loads(Path('bedtime/tests/fixtures/gltf/cube.gltf').read_text())
            (root / 'geometry.bin').write_bytes(base64.b64decode(source['buffers'][0]['uri'].split(',')[1]))
            source['buffers'][0]['uri'] = '../geometry.bin'
            texture = root / 'textures/albedo color.png'
            texture.write_bytes(png([220, 60, 30, 255]))
            source['images'][0].pop('bufferView', None)
            source['images'][0]['uri'] = '../textures/albedo%20color.png'
            path = root / 'models/scene.gltf'
            path.write_text(json.dumps(source))
            glb = root / 'cube.glb'
            glb.write_bytes(Path('bedtime/tests/fixtures/gltf/cube.glb').read_bytes())
            stl = root / 'triangle.stl'
            stl.write_text('solid triangle\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 2 0 0\nvertex 0 2 0\nendloop\nendfacet\nendsolid triangle\n')
            binary = str(Path(args.binary).resolve())
            with Application([binary, str(root)], output=output/'first', timeout=90) as app:
                state = ready(app)
                layout = next(l['entity'] for l in workspace(state)['layouts'] if l['active'])
                scene = open_model(app, layout, path)
                assert model(app, scene)['info']['triangles'] == 12
                captures.append(capture(app, '01-sidecars.png', scene))
                captures.append(capture(app, '01-canvas-hint.png'))
                rect = model(app, scene)['rect']
                assert rect == model(app,scene)['panel_rect'], 'Model canvas must fill the panel without padding'
                texts = app.inspect()['ui']['texts']
                hint = next(t['rect'] for t in texts if t['text'].startswith('Drag to orbit'))
                button = next(t['rect'] for t in texts if t['text']=='Frame All')
                assert abs(hint['max'][0] - (rect['max'][0]-8)) < 1
                assert abs(hint['max'][1] - (rect['max'][1]-6)) < 1
                assert hint['max'][1]-hint['min'][1] < button['max'][1]-button['min'][1]
                start = center(rect, app.inspect()['ui']['surfaces'][0]['zoom'])
                app.input({'type':'drag','from':start,'to':[start[0]+65,start[1]+25],'frames':12})
                captures.append(capture(app,'02-orbit.png',scene))
                assert captures[-1]['hash'] != captures[0]['hash']
                before = model(app,scene)['view']['camera']
                app.input({'type':'move','position':start}, {'type':'scroll','delta':[0,2]})
                loaded(app,scene)
                assert model(app,scene)['view']['camera']['distance'] != before['distance']
                app.input({'type':'move','position':start}, {'type':'button','button':'Right','pressed':True}, *[{'type':'move','position':[start[0]+3*i,start[1]+2*i]} for i in range(1,11)], {'type':'button','button':'Right','pressed':False})
                assert model(app,scene)['view']['camera']['target'] != before['target']
                choose(app,'Frame All')
                for mode in ['Wireframe','Wireframe overlay','Normals','Shaded']:
                    current_mode = model(app,scene)['view']['appearance']['display']
                    label = {'WireframeOverlay':'Wireframe overlay'}.get(current_mode,current_mode)
                    select(app,label,mode)
                    captures.append(capture(app,f'03-{mode.replace(" ","-")}.png',scene))
                choose(app,'Appearance')
                choose(app,'Skybox')
                escape(app)
                current_light = 'Studio'
                for light in ['Outdoor','Neutral','Kiara Dawn (HDRI)','Studio Small 08 (HDRI)']:
                    select(app,current_light,light)
                    current_light = light
                    captures.append(capture(app,'04-light-'+light.split()[0]+'.png',scene))
                # A changed sidecar refreshes without changing the root document.
                choose(app,'Appearance')
                choose(app,'Skybox')
                escape(app)
                generation = model(app,scene)['generation']
                texture.write_bytes(png([20,210,55,255]))
                wait_value(app,lambda s:any(p['entity']==scene and p['model']['generation'] > generation for p in s['previews']))
                captures.append(capture(app,'05-changed-texture.png',scene))
                # Open Code through the same route, then edit the unsaved canonical source.
                code = app.request('app.open_file',layout=layout,path=str(path),viewer='code')['panel']
                wait_value(app,lambda s:any(p['entity']==code and p['document'] for p in s['previews']))
                assert current(app,code)['document'] == current(app,scene)['document']
                area = app.request('area.split',area=current(app,scene)['area'],axis='vertical')['area']
                app.request('panel.move',panel=code,area=area)
                app.request('panel.focus',panel=code)
                app.request('app.set_settings',values={'autosave':'Never'})
                app.input({'type':'click','position':center(current(app,code)['editor_rect'], app.inspect()['ui']['surfaces'][0]['zoom'])})
                primary(app,'KeyA')
                changed = dict(source)
                changed['nodes'] = [{'mesh':0,'translation':[4,0,0]}]
                app.input({'type':'text','text':json.dumps(changed)})
                wait_value(app,lambda s:any(p['entity']==scene and p['model']['info']['minimum'][0] > 2 for p in s['previews']))
                captures.append(capture(app,'06-unsaved-source.png',scene))
                primary(app,'KeyS')
                app.request('panel.close',panel=code)
                app.request('panel.focus',panel=scene)
                choose(app,'Frame All')
                # Independent scenes coexist with the application's cube and mascot.
                other = open_model(app,layout,glb)
                other_area = app.request('area.split',area=current(app,scene)['area'],axis='horizontal')['area']
                app.request('panel.move',panel=other,area=other_area)
                app.request('panel.focus',panel=scene)
                before = model(app,scene)['view']
                cube = open_panel(app,'3D Viewport',area=area)
                bed = open_panel(app,'Bed',area=area)
                captures.append(capture(app,'07-isolation.png',scene))
                assert model(app,scene)['view'] == before
                app.request('panel.close',panel=bed)
                app.request('panel.close',panel=cube)
                triangle = open_model(app,layout,stl)
                captures.append(capture(app,'08-stl.png',triangle))
                assert model(app,triangle)['info']['triangles'] == 1
                app.request('panel.close',panel=triangle)
                app.request('panel.focus',panel=scene)
                app.request('window.resize',size=[720,850])
                choose(app,'Frame All')
                captures.append(capture(app,'09-portrait.png',scene))
                app.request('app.select_layout',slot=2)
                app.request('wait_frames',frames=5)
                assert all(not c['active'] for c in app.request('app.inspect_models')['cameras'])
                app.request('layout.activate',layout=layout)
                loaded(app,scene)
                saved_view = model(app,scene)['view']
                clean_quit(app)
            with Application([binary,str(root)],output=output/'restored',timeout=90) as app:
                ready(app)
                wait_value(app,lambda s:any(p['path']==str(path) and p['model'] and p['model']['ready'] for p in s['previews']))
                scene = next(p['entity'] for p in inspection(app)['previews'] if p['path']==str(path))
                loaded(app,scene)
                assert model(app,scene)['view'] == saved_view
                captures.append(capture(app,'10-restored.png',scene))
                for panel in list(inspection(app)['previews']):
                    app.request('panel.close',panel=panel['entity'])
                app.request('wait_frames',frames=5)
                assert app.request('app.inspect_models')['cameras'] == []
                clean_quit(app)
    finally:
        if previous is None:
            os.environ.pop('XDG_CONFIG_HOME',None)
        else:
            os.environ['XDG_CONFIG_HOME'] = previous
    for log in output.rglob('stderr.log'):
        text = log.read_text()
        assert 'panicked at' not in text and 'ERROR' not in text, log
    (output/'summary.json').write_text(json.dumps({'captures':captures},indent=2))
    print(json.dumps({'captures':captures},indent=2))


if __name__ == '__main__':
    main()
