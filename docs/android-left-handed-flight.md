# Left-handed Buckbeak flight (Android touch overlay)

The phone's touch controls are drawn by the Java overlay in the **flipendo-droid** repo
(`android/java/io/github/flipendo/spike/FlipendoActivity.java`, `class Overlay`), not by this engine. A touch that starts on the
left half of the screen becomes the movement stick, one on the right half the look drag. This engine now tells the overlay when
the player rides Buckbeak and wants a left-handed flight; the overlay then swaps the halves.

## Engine side (done, in this repo)

- `apps/hp3/src/android.rs`: JNI native `nativeLeftyFlightActive`, true while the player rides Buckbeak (the controller is in
  `PlayerFlappingHippo`) **and** `HP3_LEFTY_FLIGHT` is set to something other than `0`.
- `HP3_LEFTY_FLIGHT=1` is set from the game folder's `env.flag` (one `NAME=value` per line), like the other debugging switches.

## Overlay side (apply to `FlipendoActivity.java` in flipendo-droid)

1. Next to `nativeCreatureActive` (about line 567) declare the native:

```java
    /** True while the player rides Buckbeak and wants a left-handed flight (HP3_LEFTY_FLIGHT=1 in the game folder's env.flag): the stick and look halves swap. Rust HP3 engine only. */
    static native boolean nativeLeftyFlightActive();
```

2. Next to `creatureActive()` (about line 657) add the wrapper:

```java
    static boolean leftyFlightActive() {
        try {
            return nativeLeftyFlightActive();
        } catch (UnsatisfiedLinkError e) {
            return false;
        }
    }
```

3. Where a new touch gets its role (about line 3016, `roles.put(id, !dpadMode && x < getWidth() / 2f ? Role.STICK : Role.LOOK);`) replace that line with:

```java
                        // Riding Buckbeak with a left-handed flight (HP3_LEFTY_FLIGHT): the halves swap, the stick on the right and the look on the left.
                        boolean leftHalf = x < getWidth() / 2f;
                        roles.put(id, !dpadMode && (leftHalf != leftyFlightActive()) ? Role.STICK : Role.LOOK);
```

Nothing else changes: with the setting off, or off the flight, `leftyFlightActive()` is false and the line behaves as before.
The buttons keep their places (move them with *Settings > Control layout* if you want them on the left).

## The invisible wall

Separately from the layout, the flight corridor is widened in this engine: see `widen_flight_corridor` in
`crates/hp3-world/src/lib.rs` (`HP3_BUCKBEAK_WALL`, 1.5 by default, 1 for the original corridor).
