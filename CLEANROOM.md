# Clean-room rules

R3paro is an independent implementation of the engine that *Harry Potter and the Prisoner of Azkaban*
(KnowWonder, 2004) needs. It contains no code from Epic Games, EA or KnowWonder, and it is not derived from
any Unreal Engine source release.

## Allowed sources

1. Disassembly of HP3's retail binaries (`hppoa.exe`, `Core.dll`, `Engine.dll`, `KWGame.dll`, `GUI.dll`,
   `Fire.dll`) for interoperability. Every function reimplemented from them carries a tag directly above it:
   `// IDA Engine.dll: <decorated export name> [HP3 0x........]`, or `not exported: sub_XXXXXXXX [HP3 0x........]`
   plus how to find it again. Code written without reversing says so in the tag (`NOT yet verified against ...`).
2. HP3's shipped data: packages, bytecode, ini and localization files.
3. Public format documentation and open-source tools that read the formats (UELib / UE Explorer, umodel,
   the multimedia wiki, FFmpeg's LGPL Bink decoders), and our own GPLv3 Flipendo / zlib SurrealEngine code.
4. Black-box observation of the retail game and of the old UT2004-based build: logs, screenshots and memory
   values only.
5. Flipendo-Droid 2004 Source Code as a reference

## Forbidden

- Using `hp3-headers/` or any header set generated from or merged with Unreal Engine headers.

## Pull request checklist

- [ ] No Unreal Engine source or headers were opened while writing this change.
- [ ] Every reimplemented native carries its IDA tag.
- [ ] No game data or anything extracted from it is committed.
