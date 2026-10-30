# HP3 package format

Every layout here was worked out from HP3's own files and is checked by `hp3-pkg check <HP3 folder>`, which
reads all 156,497 exports of the 109 packages and requires each to be consumed to its last byte. Field names
follow the scripts where a script declares the class; the rest are descriptive.

## Primitives

- Integers and floats are little-endian. A *compact index* (CI) is a sign bit and 6 value bits in the first byte
  (bit 6 = more bytes follow), then 7 value bits per byte (bit 7 = more).
- A string is a CI length then Latin-1 bytes (positive) or UTF-16 units (negative), both including the NUL.
- An array is a CI count then the elements. A *lazy array* is an i32 absolute file offset just past the array,
  then the array.
- An object reference is a CI: 0 none, negative an import (`-i-1`), positive an export (`i-1`). A name is a CI
  index into the name table.

## Versions

HP3's own packages are version 129, licensee 0. Several texture, sound and mesh packages are older (99 to 128,
some with licensee 1, 28 or 29), and the layout depends on the version where noted below.

Localized packages are named `<name>.<language>_<extension>`, e.g. `alldialog.rus_uax` for the Russian
dialog; the install's language is the one whose localized packages are present.

## Default properties

Struct values inside tagged properties are themselves tag lists, except for `Vector`, `Rotator`, `Color`,
`Quat`, `Guid`, `Coords`, `Box`, `IntBox` and `FloatBox`, which are their raw members. `Plane`, `Range`,
`RangeVector` and `Matrix` are tag lists. Actors saved with `bDeleteMe` write their last struct tag without
its terminating `None`.

## Header and tables

Tag `0x9E2A83C1`, version u16, licensee u16, flags u32, then name/export/import count and offset (u32 each), a
GUID, and the generations (u32 count, then export count and name count per generation).

- Name: string, flags u32.
- Import: class package (name), class (name), outer (i32 reference), name.
- Export: class, super (CI references), outer (i32), name (CI), flags u32, size (CI), offset (CI, only if size > 0).

## Every export

1. If the export flags have `0x02000000`, a state frame: node, state node (CI), probe mask u64, latent action u32,
   and a code offset (CI) when node is not none.
2. Unless the export is a class: tagged properties up to the name `None`. A tag is a name, an info byte (type in
   bits 0-3, size code in bits 4-6, array flag in bit 7), the struct name for struct tags, the size (codes 0-4
   mean 1, 2, 4, 12, 16; 5, 6, 7 mean a following u8, u16 or i32), an array index when the array flag is set
   (except for bools, where the flag is the value), then the data.
3. The class's native data, below.

Actors saved with `bDeleteMe` set carry a second tag list after the first.

## Script objects

- Field: super field, next (CI).
- Struct: field, script text, children (CI), friendly name, a fourth CI (none in every struct), line i32, text
  position i32, script size u32, then the bytecode. The size counts bytes of the loaded form, where object and
  name references take 4 bytes, so the bytecode has to be decoded token by token to find its end.
- Function: struct, native index u16, operator precedence u8, flags u32, replication offset u16 if flag `0x40`.
- State: struct, probe mask u64, ignore mask u64, label table offset u16, state flags u32.
- Class: state, class flags u32, GUID, dependencies (class CI, deep u32, script CRC u32), package imports (names),
  within (CI), config name, hide categories (names), then the default properties as tags.
- Enum: field, names. Const: field, string. Text buffer: pos u32, top u32, string.
- Property: field, array dimension u16, element size u16, flags u32, category (name), replication offset u16 if
  flag `0x20`, then: byte property its enum, object property its class, class property class and meta class,
  array its inner property, fixed array inner and count, map key and value, struct its struct, delegate its
  function.

### Bytecode

The standard token set plus `0x03` (state variable: an object operand) and `0x15` (line number: a u16). Native
calls are `0x60`-`0x6F` with a second byte (extended index) or `0x70` and above; every call, native or not, takes
arguments up to the end-of-parameters token `0x16`. Delegate comparisons `0x3B`-`0x3E` take two expressions and
the end token. All 1,088,997 bytes of bytecode in the game decode with this table to the exact stored sizes.

## Assets

- **Palette**: colors (BGRA).
- **Texture**, **Cubemap**, **MovieTexture**: mips, each a lazy byte array, width i32, height i32, width bits u8,
  height bits u8. **FireTexture** adds sparks (8 bytes each); wet and ice textures add nothing.
- **Sound**: file type (name: `wav` or `bik`), lazy byte array. From version 127: an i32 flag, then for flag 1
  a byte array (creature and effect sounds), for flag 2 four i32s and a float array (dialog lines); then an i32.
- **Font**: from version 122, glyphs (u, v, width, height i32 and texture index u8) and textures; before, pages
  (texture and glyphs without the index) and a per-page count i32. Then kerning i32 (from version 119), a
  remap of u16 pairs, and an is-remapped i32.
- **StaticMesh**: primitive bounds (box with a validity byte, sphere), sections (flags i32, five u16), box,
  vertex stream (position and normal) + revision, color stream + revision, alpha stream + revision, UV streams
  (UVs, coordinate index, revision), index stream + revision, wireframe index stream + revision, collision model
  (CI), collision triangles (three u16 and a u8 material), collision nodes (20 bytes each), a vector, raw
  triangles (lazy: three positions, UV channel count i32, three UVs per channel, three colors, material i32,
  smoothing mask u32), internal version i32, Karma properties (CI), i32.
- **KMeshProps**: inertia tensor (6 floats), center of mass offset, then spheres, boxes, cylinders and convex
  hulls, each starting with a 4x4 matrix (convex hulls then list their vertices).
- **Polys**: count i32, max i32, then polygons: vertex count (CI), base, normal, texture U, texture V, vertices,
  flags u32, actor, material, item name, link, brush poly (CI each), lightmap scale f32.
- **Model**: primitive bounds, vectors, points, nodes, surfaces, verts (vertex, side CIs), shared side count i32,
  zone count i32 and zones, polys (CI), leaf bounds (boxes), leaf hulls (i32s), leaves (three CIs and a u64),
  lights (CIs), root outside i32, linked i32, render sections, lightmaps, lightmap textures.
  - Node: plane, zone mask u64, flags u8, seven CIs (vertex pool, surface, back, front, coplanar, collision
    bound, render bound), exclusive sphere, 16 unknown bytes, two zone bytes, vertex count u8, two leaf i32s,
    section i32, first vertex i32, lightmap i32.
  - Surface: material, flags u32, base, normal, texture U, texture V, brush poly, actor (CIs), plane, lightmap scale.
  - Render section: vertices (position, UV, lightmap UV, normal), node count i32, material, i32, flags u32, lightmap texture i32.
  - Lightmap: a u8, six CIs (surface, unknown, offset x/y, size x/y), world-to-lightmap matrix, base, X and Y axes,
    light bitmaps (light, bits, size, stride, min and max as i32s), a u8 and an i32.
  - Lightmap texture: a u8, lightmap indices (i32s), a u64, an i32, two empty lazy arrays, a u8, two floats, an i32.
- **ConvexVolume**: primitive bounds, faces (plane and vertices), edges (four i32s), bounds.
- **Level**: actor count i32, max i32, actors (CIs), URL (protocol, host, map, portal, options, port, valid),
  model, time f32, first deleted, 16 text blocks, travel info (string pairs).
- **StaticMeshInstance**: vertex colors, an i32, lights (light, one visibility bit per vertex, apply i32).
- **TerrainInfo** (after its tags): sectors, vertices, sectors X/Y i32, face normal pairs, to-world and
  to-heightmap coordinate frames (origin and three axes), heightmap X/Y i32, vertex colors.
- **TerrainSector**: terrain info, quads X/Y, offset X/Y (i32s), box, lights (light and visibility bits).
- **LodMesh header** (skeletal and vertex meshes): primitive bounds, version i32, vertex count i32, packed vertices
  (u32s), textures, scale, origin, rotation origin (three i32s), face levels (u16s), faces (three wedge u16s and
  a material u16), collapse wedges (u16s), wedges (vertex u16, u, v), materials (flags u32, texture i32), scale max
  f32, five LOD floats, 62 bytes that are zero in every mesh.
- **SkeletalMesh**: header, bones (name, flags u32, orientation quaternion, position, length, size, child count
  i32, parent i32), a CI, depth i32, an i32, an empty i32 array, LOD models, an empty i32 array, then lazy arrays of
  points, wedges, triangles (three u16s, two u8s, u32), influences (weight f32, vertex u16, bone u16), collapse
  wedges (u16s) and face levels (u16s), then two i32s.
  - LOD model: skinning data (u32s), skin points (16 bytes each), dynamic wedge count i32, smooth and rigid
    sections (nine u16s each), smooth and rigid index buffers (u16s + revision), a vertex stream (i32, empty array,
    two i32s), lazy influences, wedges, faces and points, then distance factor f32, hysteresis f32, shared vertex
    count i32, maximum influences i32 and two i32s.
- **MeshAnimation**: version i32, bones (name, flags u32, parent i32), motions (root speed, track time f32, start
  bone i32, flags u32, bone indices, tracks, root track, an empty i32 array), sequences.
  - Track: flags u32, rotations (quaternions), positions, times (f32s).
  - Sequence: a float, name, groups (names), start frame i32, frame count i32, notifies (time f32, function name,
    object), rate f32.
- **VertMesh**: header, an empty u32 array, an i32, an empty u32 array, sequences, packed vertices of all frames,
  vertices per frame i32, frame count i32, per-frame boxes and spheres.
