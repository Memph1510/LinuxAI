"""LinuxAI's engine inside Blender: inspect, change, render and export a scene.

One engine, two surfaces:
  - headless jobs, run by the Rust blender organ:
        blender -b --factory-startup --disable-autoexec [FILE.blend] --python linuxai_blender.py -- JOB.json RESULT.json
  - the live bridge in the human's open Blender (scripts/startup/linuxai_bridge.py), which calls `run_ops` and
    `inspect` on the scene they have open.

This file runs inside Blender's own Python (bpy), the only way to drive Blender. Everything around it (staging
copies, checking, recording, deciding) is the Rust organ and the AI.

A job: {"op": "inspect"} | {"op": "apply", "ops": [...], "save_as": PATH, "preview": {...}?}
       | {"op": "render", "output": PATH, "frame"?, "width"?, "height"?, "samples"?, "engine"?}
       | {"op": "export", "format": "glb|gltf|fbx|obj|stl|ply|usd", "output": PATH, "selection"?: [names]}
The original file is never saved over: `apply` writes a new file (save_as), and refuses to write onto the file
that was opened.
"""
from __future__ import annotations

import json
import math
import os
import sys
import time
import traceback

import bpy
import mathutils

MAX_OBJECTS = 300


# ------------------------------------------------------------------------------------------------
# Looking at a scene
# ------------------------------------------------------------------------------------------------

def _r(v, n=3):
    return [round(x, n) for x in v]


def _obj(o) -> dict:
    d = {"name": o.name, "type": o.type, "location": _r(o.location), "rotation": _r([math.degrees(a) for a in o.rotation_euler], 1),
         "scale": _r(o.scale), "dimensions": _r(o.dimensions), "collections": [c.name for c in o.users_collection]}
    if o.parent:
        d["parent"] = o.parent.name
    if o.hide_get() or o.hide_render:
        d["hidden"] = True
    mats = [s.material.name for s in getattr(o, "material_slots", []) if s.material]
    if mats:
        d["materials"] = mats
    if o.modifiers:
        d["modifiers"] = [f"{m.type.lower()}:{m.name}" for m in o.modifiers]
    if o.type == "MESH":
        d["vertices"], d["faces"] = len(o.data.vertices), len(o.data.polygons)
        if any(abs(s - 1) > 1e-4 for s in o.scale):
            d["unapplied_scale"] = True
    elif o.type == "LIGHT":
        d["light"] = {"kind": o.data.type.lower(), "energy": round(o.data.energy, 2), "color": _r(o.data.color)}
    elif o.type == "CAMERA":
        d["camera"] = {"lens": round(o.data.lens, 1), "active": bpy.context.scene.camera == o}
    elif o.type == "FONT":
        d["text"] = o.data.body[:200]
    if o.animation_data and o.animation_data.action:
        d["animated"] = True
    return d


def inspect() -> dict:
    s = bpy.context.scene
    objs = list(s.objects)
    images = []
    for im in bpy.data.images:
        if im.source == "FILE" and im.filepath:
            p = bpy.path.abspath(im.filepath)
            images.append({"name": im.name, "path": p, "missing": not im.packed_file and not os.path.exists(p)})
    return {
        "file": bpy.data.filepath or None,
        "saved_with": ".".join(map(str, bpy.data.version)) if bpy.data.filepath else None,
        "scene": s.name,
        "render": {"engine": s.render.engine, "width": s.render.resolution_x, "height": s.render.resolution_y,
                   "percent": s.render.resolution_percentage, "fps": s.render.fps,
                   "frames": [s.frame_start, s.frame_end], "camera": s.camera.name if s.camera else None},
        "units": {"system": s.unit_settings.system, "scale": s.unit_settings.scale_length},
        "objects": [_obj(o) for o in objs[:MAX_OBJECTS]],
        "objects_total": len(objs),
        "collections": [c.name for c in bpy.data.collections],
        "materials": [m.name for m in bpy.data.materials if m.users],
        "images": images[:100],
        # Scripts inside a file are outside content: listed so the AI knows, never run (auto-run stays off).
        "scripts_in_file": [t.name for t in bpy.data.texts] + (["(drivers)"] if any(
            o.animation_data and o.animation_data.drivers for o in objs) else []),
    }


# ------------------------------------------------------------------------------------------------
# Changing a scene: a list of small, checked operations
# ------------------------------------------------------------------------------------------------

class OpError(Exception):
    pass


def _get(name):
    o = bpy.data.objects.get(name)
    if o is None:
        raise OpError(f"no object named {name!r}")
    return o


def _vec(v, default=(0, 0, 0)):
    if v is None:
        return mathutils.Vector(default)
    if isinstance(v, (int, float)):
        return mathutils.Vector((v, v, v))
    if len(v) != 3:
        raise OpError(f"expected [x, y, z], got {v!r}")
    return mathutils.Vector([float(x) for x in v])


def _rot(v):
    return mathutils.Euler([math.radians(float(x)) for x in _vec(v)], "XYZ")


def _color(c, alpha=True):
    if isinstance(c, str):
        h = c.lstrip("#")
        if len(h) != 6:
            raise OpError(f"color {c!r}: use #rrggbb or [r, g, b] from 0 to 1")
        # sRGB hex to the linear values Blender stores.
        lin = [((int(h[i:i + 2], 16) / 255) ** 2.2) for i in (0, 2, 4)]
    else:
        lin = [float(x) for x in c][:3]
    return lin + [1.0] if alpha else lin


def _link(o, collection: str | None = None):
    coll = bpy.context.scene.collection
    if collection:
        coll = bpy.data.collections.get(collection) or bpy.data.collections.new(collection)
        if coll.name not in bpy.context.scene.collection.children and coll is not bpy.context.scene.collection:
            try:
                bpy.context.scene.collection.children.link(coll)
            except RuntimeError:
                pass
    coll.objects.link(o)


PRIMITIVES = {"cube", "plane", "uv_sphere", "sphere", "ico_sphere", "cylinder", "cone", "torus", "monkey", "circle", "grid"}


def op_add(a):
    kind = a.get("type", "cube").lower()
    size = float(a.get("size", 2.0))
    if kind in PRIMITIVES:
        import bmesh
        bm = bmesh.new()
        r = size / 2
        if kind == "cube":
            bmesh.ops.create_cube(bm, size=size)
        elif kind == "plane" or kind == "grid":
            bmesh.ops.create_grid(bm, x_segments=int(a.get("segments", 1)), y_segments=int(a.get("segments", 1)), size=r)
        elif kind in ("uv_sphere", "sphere"):
            bmesh.ops.create_uvsphere(bm, u_segments=int(a.get("segments", 32)), v_segments=int(a.get("rings", 16)), radius=r)
        elif kind == "ico_sphere":
            bmesh.ops.create_icosphere(bm, subdivisions=int(a.get("subdivisions", 2)), radius=r)
        elif kind in ("cylinder", "cone"):
            bmesh.ops.create_cone(bm, cap_ends=True, segments=int(a.get("segments", 32)), radius1=r,
                                  radius2=0 if kind == "cone" else r, depth=float(a.get("depth", size)))
        elif kind == "circle":
            bmesh.ops.create_circle(bm, cap_ends=True, segments=int(a.get("segments", 32)), radius=r)
        elif kind == "monkey":
            bmesh.ops.create_monkey(bm)
        elif kind == "torus":
            bm.free()
            bpy.ops.mesh.primitive_torus_add(major_radius=r, minor_radius=float(a.get("thickness", r / 4)))
            o = bpy.context.active_object
            o.name = a.get("name") or "Torus"
            _place(o, a)
            return o.name
        mesh = bpy.data.meshes.new(a.get("name") or kind.title())
        bm.to_mesh(mesh)
        bm.free()
        o = bpy.data.objects.new(a.get("name") or kind.title(), mesh)
    elif kind == "empty":
        o = bpy.data.objects.new(a.get("name") or "Empty", None)
    elif kind == "text":
        curve = bpy.data.curves.new(a.get("name") or "Text", "FONT")
        curve.body = str(a.get("text", "Text"))
        curve.size = float(a.get("size", 1.0))
        curve.extrude = float(a.get("depth", 0.0))
        o = bpy.data.objects.new(a.get("name") or "Text", curve)
    else:
        raise OpError(f"add: unknown type {kind!r} (cube, plane, sphere, ico_sphere, cylinder, cone, torus, monkey, "
                      "circle, grid, empty, text; lights and cameras have their own ops)")
    _link(o, a.get("collection"))
    _place(o, a)
    return o.name


def _place(o, a):
    if "location" in a:
        o.location = _vec(a["location"])
    if "rotation" in a:
        o.rotation_euler = _rot(a["rotation"])
    if "scale" in a:
        o.scale = _vec(a["scale"], (1, 1, 1))


def op_delete(a):
    names = a["name"] if isinstance(a["name"], list) else [a["name"]]
    for n in names:
        bpy.data.objects.remove(_get(n), do_unlink=True)


def op_transform(a):
    o = _get(a["name"])
    if a.get("relative"):
        if "location" in a:
            o.location += _vec(a["location"])
        if "rotation" in a:
            r = _vec(a["rotation"])
            o.rotation_euler = mathutils.Euler([o.rotation_euler[i] + math.radians(r[i]) for i in range(3)], "XYZ")
        if "scale" in a:
            s = _vec(a["scale"], (1, 1, 1))
            o.scale = mathutils.Vector([o.scale[i] * s[i] for i in range(3)])
    else:
        _place(o, a)
    if "dimensions" in a:
        o.dimensions = _vec(a["dimensions"])


def op_rename(a):
    o = _get(a["name"])
    o.name = a["to"]
    return o.name


def op_material(a):
    name = a.get("name") or "Material"
    m = bpy.data.materials.get(name) or bpy.data.materials.new(name)
    if m.node_tree is None:
        m.use_nodes = True
    p = next((n for n in m.node_tree.nodes if n.type == "BSDF_PRINCIPLED"), None)
    if p is None:
        raise OpError(f"material {name!r} has no Principled BSDF node to set")
    if "color" in a:
        p.inputs["Base Color"].default_value = _color(a["color"])
        m.diffuse_color = _color(a["color"])
    for key, sock in (("metallic", "Metallic"), ("roughness", "Roughness"), ("alpha", "Alpha"),
                      ("emission_strength", "Emission Strength"), ("transmission", "Transmission Weight")):
        if key in a and sock in p.inputs:
            p.inputs[sock].default_value = float(a[key])
    if "emission" in a and "Emission Color" in p.inputs:
        p.inputs["Emission Color"].default_value = _color(a["emission"])
        if "emission_strength" not in a:
            p.inputs["Emission Strength"].default_value = 1.0
    for n in (a.get("assign_to") or []):
        o = _get(n)
        if not hasattr(o.data, "materials"):
            raise OpError(f"{n!r} can't have a material")
        if a.get("replace", True):
            o.data.materials.clear()
        o.data.materials.append(m)
    return m.name


def op_light(a):
    kind = a.get("kind", "point").upper()
    data = bpy.data.lights.new(a.get("name") or kind.title(), kind)
    data.energy = float(a.get("energy", {"SUN": 3, "POINT": 1000, "SPOT": 1000, "AREA": 500}.get(kind, 1000)))
    if "color" in a:
        data.color = _color(a["color"], alpha=False)
    if kind == "AREA" and "size" in a:
        data.size = float(a["size"])
    o = bpy.data.objects.new(a.get("name") or kind.title(), data)
    _link(o, a.get("collection"))
    _place(o, a)
    if "look_at" in a:
        _look_at(o, a["look_at"])
    return o.name


def _look_at(o, target):
    t = _get(target).matrix_world.translation if isinstance(target, str) else _vec(target)
    bpy.context.view_layer.update()
    direction = t - o.matrix_world.translation
    o.rotation_euler = direction.to_track_quat("-Z", "Y").to_euler()


def op_camera(a):
    o = bpy.data.objects.get(a["name"]) if a.get("name") else None
    if o is None or o.type != "CAMERA":
        o = bpy.data.objects.new(a.get("name") or "Camera", bpy.data.cameras.new(a.get("name") or "Camera"))
        _link(o, a.get("collection"))
    _place(o, a)
    if "lens" in a:
        o.data.lens = float(a["lens"])
    if "look_at" in a:
        _look_at(o, a["look_at"])
    if a.get("active", True):
        bpy.context.scene.camera = o
    return o.name


def op_render_settings(a):
    s = bpy.context.scene
    if "engine" in a:
        s.render.engine = {"cycles": "CYCLES", "eevee": "BLENDER_EEVEE", "workbench": "BLENDER_WORKBENCH"}.get(
            str(a["engine"]).lower(), a["engine"])
    for k, attr in (("width", "resolution_x"), ("height", "resolution_y"), ("percent", "resolution_percentage"), ("fps", "fps")):
        if k in a:
            setattr(s.render, attr, int(a[k]))
    if "samples" in a:
        s.cycles.samples = int(a["samples"])
        s.eevee.taa_render_samples = int(a["samples"])
    if "frame_start" in a:
        s.frame_start = int(a["frame_start"])
    if "frame_end" in a:
        s.frame_end = int(a["frame_end"])


def op_world(a):
    s = bpy.context.scene
    w = s.world or bpy.data.worlds.new("World")
    s.world = w
    if w.node_tree is None:
        w.use_nodes = True
    bg = next((n for n in w.node_tree.nodes if n.type == "BACKGROUND"), None)
    if bg is None:
        raise OpError("the world has no Background node")
    if "color" in a:
        bg.inputs["Color"].default_value = _color(a["color"])
    if "strength" in a:
        bg.inputs["Strength"].default_value = float(a["strength"])


MODIFIERS = {"bevel": "BEVEL", "subsurf": "SUBSURF", "subdivision": "SUBSURF", "mirror": "MIRROR", "array": "ARRAY",
             "solidify": "SOLIDIFY", "decimate": "DECIMATE", "remesh": "REMESH", "boolean": "BOOLEAN",
             "weld": "WELD", "triangulate": "TRIANGULATE", "smooth": "SMOOTH", "wireframe": "WIREFRAME"}


def op_modifier(a):
    o = _get(a["name"])
    kind = MODIFIERS.get(str(a.get("kind", "")).lower())
    if kind is None:
        raise OpError(f"modifier kind: one of {', '.join(sorted(MODIFIERS))}")
    m = o.modifiers.new(a.get("label") or a["kind"].title(), kind)
    for k, v in (a.get("settings") or {}).items():
        if k == "object":
            v = _get(v)
        if not hasattr(m, k):
            raise OpError(f"{a['kind']} modifier has no setting {k!r}")
        setattr(m, k, v)
    return m.name


def op_apply_transforms(a):
    names = [o.name for o in bpy.context.scene.objects if o.type == "MESH"] if a.get("all") else (
        a["name"] if isinstance(a.get("name"), list) else [a["name"]])
    for n in names:
        o = _get(n)
        if o.type != "MESH":
            continue
        mw = o.matrix_basis.copy()
        loc, rot, scale = mw.decompose()
        keep_loc = mathutils.Matrix.Translation(loc)
        o.data.transform(rot.to_matrix().to_4x4() @ mathutils.Matrix.Diagonal(scale.to_4d()))
        o.matrix_basis = keep_loc
        for c in o.children:
            c.matrix_parent_inverse = mathutils.Matrix.Identity(4)


def op_recalc_normals(a):
    import bmesh
    o = _get(a["name"])
    bm = bmesh.new()
    bm.from_mesh(o.data)
    bmesh.ops.recalc_face_normals(bm, faces=bm.faces)
    bm.to_mesh(o.data)
    bm.free()


def op_shade(a):
    o = _get(a["name"])
    smooth = bool(a.get("smooth", True))
    for p in o.data.polygons:
        p.use_smooth = smooth


def op_parent(a):
    o = _get(a["name"])
    parent = _get(a["to"]) if a.get("to") else None
    mw = o.matrix_world.copy()
    o.parent = parent
    o.matrix_world = mw


def op_collection(a):
    o = _get(a["name"])
    for c in list(o.users_collection):
        c.objects.unlink(o)
    _link(o, a["to"])


def op_keyframe(a):
    o = _get(a["name"])
    frame = int(a["frame"])
    prop = a.get("property", "location")
    if prop not in ("location", "rotation", "scale"):
        raise OpError("keyframe property: location, rotation or scale")
    if "value" in a:
        if prop == "rotation":
            o.rotation_euler = _rot(a["value"])
        else:
            setattr(o, prop, _vec(a["value"]))
    o.keyframe_insert(data_path="rotation_euler" if prop == "rotation" else prop, frame=frame)


IMPORTERS = {
    "glb": lambda p: bpy.ops.import_scene.gltf(filepath=p), "gltf": lambda p: bpy.ops.import_scene.gltf(filepath=p),
    "fbx": lambda p: bpy.ops.import_scene.fbx(filepath=p), "obj": lambda p: bpy.ops.wm.obj_import(filepath=p),
    "stl": lambda p: bpy.ops.wm.stl_import(filepath=p), "ply": lambda p: bpy.ops.wm.ply_import(filepath=p),
    "usd": lambda p: bpy.ops.wm.usd_import(filepath=p), "usda": lambda p: bpy.ops.wm.usd_import(filepath=p),
    "usdc": lambda p: bpy.ops.wm.usd_import(filepath=p), "usdz": lambda p: bpy.ops.wm.usd_import(filepath=p),
}


def op_import(a):
    path = os.path.expanduser(a["path"])
    fmt = (a.get("format") or os.path.splitext(path)[1].lstrip(".")).lower()
    if fmt not in IMPORTERS:
        raise OpError(f"import: {fmt!r} isn't supported ({', '.join(IMPORTERS)})")
    if not os.path.exists(path):
        raise OpError(f"import: no file {path}")
    before = set(bpy.data.objects.keys())
    IMPORTERS[fmt](path)
    added = sorted(set(bpy.data.objects.keys()) - before)
    if a.get("location") is not None:
        for n in added:
            o = bpy.data.objects[n]
            if o.parent is None:
                o.location += _vec(a["location"])
    return added


def op_image_plane(a):
    """A picture (e.g. from the image organ) as a textured plane: reference images, posters, decals."""
    path = os.path.expanduser(a["path"])
    img = bpy.data.images.load(path, check_existing=True)
    w, h = img.size
    height = float(a.get("height", 2.0))
    width = height * (w / h if h else 1)
    import bmesh
    bm = bmesh.new()
    bmesh.ops.create_grid(bm, x_segments=1, y_segments=1, size=0.5)
    uv = bm.loops.layers.uv.new()
    for f in bm.faces:
        for loop in f.loops:
            loop[uv].uv = (loop.vert.co.x + 0.5, loop.vert.co.y + 0.5)
    mesh = bpy.data.meshes.new(a.get("name") or img.name)
    bm.to_mesh(mesh)
    bm.free()
    o = bpy.data.objects.new(a.get("name") or img.name, mesh)
    o.scale = (width, height, 1)
    m = bpy.data.materials.new(o.name)
    if m.node_tree is None:
        m.use_nodes = True
    p = next(n for n in m.node_tree.nodes if n.type == "BSDF_PRINCIPLED")
    tex = m.node_tree.nodes.new("ShaderNodeTexImage")
    tex.image = img
    m.node_tree.links.new(tex.outputs["Color"], p.inputs["Base Color"])
    m.node_tree.links.new(tex.outputs["Alpha"], p.inputs["Alpha"])
    mesh.materials.append(m)
    _link(o, a.get("collection"))
    o.rotation_euler = _rot(a.get("rotation", [90, 0, 0]))
    if "location" in a:
        o.location = _vec(a["location"])
    return o.name


def op_frame(a):
    bpy.context.scene.frame_set(int(a["frame"]))


def op_python(a):
    """Blender Python the AI wrote, for what the other ops can't do. The organ only runs it after the check the
    AI's commands get (it can do anything the user can), and only on a staged copy or with the human's OK live."""
    env = {"bpy": bpy, "mathutils": mathutils, "math": math, "result": None}
    exec(compile(a["code"], "<linuxai op>", "exec"), env)
    r = env.get("result")
    return r if isinstance(r, (str, int, float, list, dict, bool)) or r is None else str(r)


OPS = {"add": op_add, "delete": op_delete, "transform": op_transform, "rename": op_rename, "material": op_material,
       "light": op_light, "camera": op_camera, "render_settings": op_render_settings, "world": op_world,
       "modifier": op_modifier, "apply_transforms": op_apply_transforms, "recalc_normals": op_recalc_normals,
       "shade": op_shade, "parent": op_parent, "collection": op_collection, "keyframe": op_keyframe,
       "import": op_import, "image_plane": op_image_plane, "frame": op_frame, "python": op_python}


def run_ops(ops: list) -> list:
    """Run each op in order; stop at the first that fails (what came before it stays done, and is reported)."""
    done = []
    for i, a in enumerate(ops):
        if not isinstance(a, dict) or a.get("op") not in OPS:
            raise OpError(f"step {i}: op must be one of {', '.join(OPS)}")
        try:
            r = OPS[a["op"]](a)
        except OpError as e:
            raise OpError(f"step {i} ({a['op']}): {e}") from None
        except (KeyError, TypeError, ValueError, AttributeError, RuntimeError) as e:
            raise OpError(f"step {i} ({a['op']}): {type(e).__name__}: {e}") from None
        done.append({"op": a["op"], **({"result": r} if r is not None else {})})
    return done


# ------------------------------------------------------------------------------------------------
# Rendering, checking and exporting
# ------------------------------------------------------------------------------------------------

def render(output: str, frame=None, width=None, height=None, samples=None, engine=None, denoise=None) -> dict:
    s = bpy.context.scene
    if s.camera is None:
        raise OpError("the scene has no camera to render from (add one with the camera op)")
    keep = (s.render.engine, s.render.resolution_x, s.render.resolution_y, s.render.resolution_percentage,
            s.render.filepath, s.render.image_settings.file_format, s.cycles.use_denoising)
    try:
        if engine:
            op_render_settings({"engine": engine})
        if width:
            s.render.resolution_x = int(width)
        if height:
            s.render.resolution_y = int(height)
        if width or height:
            s.render.resolution_percentage = 100
        if s.render.engine == "CYCLES":
            s.cycles.device = "CPU" if os.environ.get("LINUXAI_BLENDER_GPU") != "1" else s.cycles.device
            if samples:
                s.cycles.samples = int(samples)
            if denoise is not None:
                s.cycles.use_denoising = bool(denoise)
        elif samples:
            s.eevee.taa_render_samples = int(samples)
        if frame is not None:
            s.frame_set(int(frame))
        s.render.filepath = output
        s.render.image_settings.file_format = "PNG"
        t = time.time()
        bpy.ops.render.render(write_still=True)
        return {"path": output, "seconds": round(time.time() - t, 1), "engine": s.render.engine,
                "size": [s.render.resolution_x, s.render.resolution_y], "check": picture_check(output)}
    finally:
        (s.render.engine, s.render.resolution_x, s.render.resolution_y, s.render.resolution_percentage,
         s.render.filepath, s.render.image_settings.file_format, s.cycles.use_denoising) = keep


def picture_check(path: str) -> dict:
    """Is there a picture there: not all one color (a black frame means no light, or nothing in view)."""
    import numpy as np
    img = bpy.data.images.load(path, check_existing=False)
    try:
        px = np.empty(len(img.pixels), dtype=np.float32)
        img.pixels.foreach_get(px)
        rgb = px.reshape(-1, 4)[:, :3]
        mean, std = float(rgb.mean()), float(rgb.std())
    finally:
        bpy.data.images.remove(img)
    verdict = "ok" if std > 0.01 else ("black" if mean < 0.02 else "flat")
    return {"verdict": verdict, "mean": round(mean, 3), "contrast": round(std, 3)}


EXPORTERS = {
    "glb": lambda p, sel: bpy.ops.export_scene.gltf(filepath=p, export_format="GLB", use_selection=sel),
    "gltf": lambda p, sel: bpy.ops.export_scene.gltf(filepath=p, export_format="GLTF_SEPARATE", use_selection=sel),
    "fbx": lambda p, sel: bpy.ops.export_scene.fbx(filepath=p, use_selection=sel),
    "obj": lambda p, sel: bpy.ops.wm.obj_export(filepath=p, export_selected_objects=sel),
    "stl": lambda p, sel: bpy.ops.wm.stl_export(filepath=p, export_selected_objects=sel),
    "ply": lambda p, sel: bpy.ops.wm.ply_export(filepath=p, export_selected_objects=sel),
    "usd": lambda p, sel: bpy.ops.wm.usd_export(filepath=p, selected_objects_only=sel),
}


def export(fmt: str, output: str, selection=None) -> dict:
    fmt = fmt.lower()
    if fmt not in EXPORTERS:
        raise OpError(f"export: one of {', '.join(EXPORTERS)}")
    if selection:
        for o in bpy.context.scene.objects:
            o.select_set(o.name in selection)
    EXPORTERS[fmt](output, bool(selection))
    if not os.path.exists(output):
        raise OpError("the exporter finished but wrote no file")
    return {"path": output, "bytes": os.path.getsize(output)}


# ------------------------------------------------------------------------------------------------
# A headless job
# ------------------------------------------------------------------------------------------------

def job(j: dict) -> dict:
    op = j.get("op")
    opened = bpy.data.filepath
    if op == "inspect":
        return {"ok": True, "scene": inspect()}
    if op == "apply":
        save_as = os.path.abspath(j["save_as"])
        if opened and os.path.abspath(opened) == save_as:
            raise OpError("refusing to save over the original; changes go to a new file")
        before = inspect()
        done = run_ops(j.get("ops") or [])
        bpy.ops.wm.save_as_mainfile(filepath=save_as, copy=True, relative_remap=True, compress=True)
        out = {"ok": True, "done": done, "saved": save_as, "before": before}
        if j.get("preview") and bpy.context.scene.camera:
            p = j["preview"]
            out["preview"] = render(p["output"], width=p.get("width", 480), height=p.get("height", 270),
                                    samples=p.get("samples", 16), engine=p.get("engine", "CYCLES"), denoise=False)
        return out
    if op == "render":
        return {"ok": True, **render(j["output"], j.get("frame"), j.get("width"), j.get("height"), j.get("samples"),
                                     j.get("engine"), j.get("denoise"))}
    if op == "export":
        return {"ok": True, **export(j["format"], j["output"], j.get("selection"))}
    raise OpError(f"unknown job op {op!r}")


def main():
    argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    if len(argv) != 2:
        print("usage: blender -b [FILE] --python linuxai_blender.py -- JOB.json RESULT.json", file=sys.stderr)
        sys.exit(2)
    with open(argv[0]) as f:
        j = json.load(f)
    try:
        r = job(j)
    except OpError as e:
        r = {"ok": False, "error": str(e)}
    except Exception as e:  # noqa: BLE001 - anything else is reported, not a crash with no answer
        r = {"ok": False, "error": f"{type(e).__name__}: {e}", "trace": traceback.format_exc()[-1500:]}
    with open(argv[1], "w") as f:
        json.dump(r, f)


if __name__ == "__main__":
    main()
