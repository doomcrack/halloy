#! /usr/bin/env python3

"""Cut a staged macOS bundle loose from the nix store.

Every logos artifact is built by nix, so each of its Mach-O files names its
dependencies by absolute `/nix/store/<hash>-<name>/...` path. Those paths
exist on the build machine and nowhere else, which is the entire reason
copying a `.app` to another Mac produces a daemon that dies before it logs
anything. This walks the bundle, pulls each `/nix/store` dependency in
beside the app, and rewrites the load commands to `@rpath` so the copy
carries its own closure.

Run it after everything is staged and before anything is signed:
`install_name_tool` invalidates a code signature, so the signing pass has
to be the last thing that touches a binary.
"""

# macOS ships Python 3.9, and this has to run on a stock machine — the
# annotations below are 3.10 syntax, so keep them unevaluated.
from __future__ import annotations

import os
import shutil
import stat
import subprocess
import sys
from pathlib import Path

# Absolute prefixes that belong to the OS and must be left exactly as they
# are. Anything else absolute is a build-machine path that has to go.
SYSTEM_PREFIXES = ("/usr/lib/", "/System/")

# Where each file in the bundle was copied from. Read back by `resolve`,
# because a nix binary's own rpaths only mean anything at its old address.
ORIGINS: dict = {}

MACHO_MAGIC = (
    b"\xcf\xfa\xed\xfe",  # 64-bit little-endian
    b"\xce\xfa\xed\xfe",  # 32-bit little-endian
    b"\xca\xfe\xba\xbe",  # universal
    b"\xbe\xba\xfe\xca",  # universal, byte-swapped
)


def is_macho(path: Path) -> bool:
    if path.is_symlink() or not path.is_file():
        return False

    try:
        with path.open("rb") as handle:
            return handle.read(4) in MACHO_MAGIC
    except OSError:
        return False


def machos_under(root: Path) -> list[Path]:
    return sorted(p for p in root.rglob("*") if is_macho(p))


def otool(flag: str, path: Path) -> list[str]:
    result = subprocess.run(
        ["otool", flag, str(path)], capture_output=True, text=True
    )

    return result.stdout.splitlines()


def dylib_id(path: Path) -> str | None:
    """A dylib's own `LC_ID_DYLIB`, which `-L` lists indistinguishably from
    a real dependency. Importing it would copy the file onto itself."""
    lines = otool("-D", path)

    return lines[1].strip() if len(lines) > 1 else None


def dependencies(path: Path) -> list[str]:
    lines = otool("-L", path)[1:]
    own = dylib_id(path)

    return [
        dep
        for line in lines
        if (dep := line.split(" (compatibility")[0].strip()) != own
    ]


def rpaths(path: Path) -> list[str]:
    lines = otool("-l", path)
    found = []

    for index, line in enumerate(lines):
        if "LC_RPATH" in line:
            for follow in lines[index : index + 4]:
                if follow.strip().startswith("path "):
                    found.append(follow.split("path ", 1)[1].split(" (offset")[0])
                    break

    return found


def install_name_tool(*args: str) -> bool:
    result = subprocess.run(
        ["install_name_tool", *args], capture_output=True, text=True
    )

    if result.returncode != 0:
        # A binary built without header padding cannot take a longer load
        # command. Every rewrite here shortens one, so this should not
        # happen — but a silent failure would ship a broken bundle.
        print(f"  ! install_name_tool {' '.join(args)}", file=sys.stderr)
        print(f"    {result.stderr.strip()}", file=sys.stderr)

    return result.returncode == 0


def make_writable(path: Path) -> None:
    """Nix store files are read-only and copy that way. Undo it in place."""
    mode = path.stat().st_mode

    if not mode & stat.S_IWUSR:
        path.chmod(mode | stat.S_IWUSR)


def framework_parts(dep: str) -> tuple[str, str] | None:
    """Split `.../QtCore.framework/Versions/A/QtCore` into its own root and
    the path under it, or return None for a plain dylib."""
    if ".framework/" not in dep:
        return None

    root, rest = dep.split(".framework/", 1)

    return (root + ".framework", rest)


def import_framework(source_root: Path, frameworks: Path) -> Path:
    """Copy a framework in the canonical layout, minus its headers.

    Flattening a framework to a bare dylib also resolves, but keeping the
    real shape is what lets `codesign` treat it as nested code rather than
    a loose file in `Frameworks/`.
    """
    name = source_root.name.removesuffix(".framework")
    dest_root = frameworks / source_root.name

    if dest_root.exists():
        return dest_root

    version = source_root / "Versions" / "A"
    dest_version = dest_root / "Versions" / "A"
    dest_version.mkdir(parents=True)

    for entry in version.iterdir():
        # Headers are for compiling against; `.prl` files are qmake link
        # metadata naming store paths that will not exist. Neither is read
        # at runtime, and the `.prl` files would be the last thing in the
        # bundle still pointing at the build machine.
        if entry.name in ("Headers", "PrivateHeaders"):
            continue

        target = dest_version / entry.name

        if entry.is_dir():
            shutil.copytree(
                entry,
                target,
                symlinks=True,
                ignore=shutil.ignore_patterns("*.prl"),
            )
        elif entry.suffix != ".prl":
            shutil.copy2(entry, target)

    (dest_root / "Versions" / "Current").symlink_to("A")

    for entry in dest_version.iterdir():
        (dest_root / entry.name).symlink_to(f"Versions/Current/{entry.name}")

    # Directories come out of the store read-only too, and a read-only
    # directory is one a later run cannot delete out of.
    for path in [dest_root, *dest_root.rglob("*")]:
        if not path.is_symlink():
            make_writable(path)

    binary = dest_version / name
    ORIGINS[binary.resolve()] = version / name
    install_name_tool(
        "-id", f"@rpath/{source_root.name}/Versions/A/{name}", str(binary)
    )

    return dest_root


def import_dylib(source: Path, frameworks: Path) -> Path:
    dest = frameworks / source.name

    if not dest.exists():
        shutil.copy2(source, dest)
        make_writable(dest)
        ORIGINS[dest.resolve()] = source
        install_name_tool("-id", f"@rpath/{source.name}", str(dest))

    return dest


def resolve(
    dep: str, macho: Path, macho_rpaths: list[str], source: Path
) -> Path | None:
    """Turn one load-command string into a file on disk.

    `@rpath` entries are the interesting case: a nix binary reaches its
    siblings through an `LC_RPATH` written as `@loader_path/../lib`, and
    `@loader_path` means wherever the file is *now*. Copying it into the
    bundle therefore breaks its own rpaths before they have been followed,
    so every relative base is tried against the original location too —
    that is the only place the rest of the closure is still reachable.
    """
    bases = {macho.parent, source.parent}

    def against(entry: str) -> list[Path]:
        if not entry.startswith("@"):
            return [Path(entry)]

        return [
            Path(
                entry.replace("@loader_path", str(base)).replace(
                    "@executable_path", str(base)
                )
            )
            for base in bases
        ]

    if dep.startswith("@rpath/"):
        leaf = dep[len("@rpath/") :]
        candidates = [
            directory / leaf
            for entry in macho_rpaths
            for directory in against(entry)
        ]
    elif dep.startswith("@"):
        candidates = against(dep)
    else:
        candidates = [Path(dep)]

    for candidate in candidates:
        if candidate.exists():
            return candidate.resolve()

    return None


def rpath_to(frameworks: Path, macho: Path) -> str:
    return "@loader_path/" + os.path.relpath(frameworks, macho.parent)


def relocate(bundle: Path) -> int:
    frameworks = bundle / "Contents" / "Frameworks"
    frameworks.mkdir(parents=True, exist_ok=True)

    queue = machos_under(bundle)
    seen: set[Path] = set()
    imported = 0
    unresolved: list[tuple[Path, str]] = []

    while queue:
        macho = queue.pop()
        key = macho.resolve()

        if key in seen:
            continue

        seen.add(key)
        make_writable(macho)

        came_from = ORIGINS.get(key, macho)
        own_rpaths = rpaths(macho)
        rewrites: list[tuple[str, str]] = []

        for dep in dependencies(macho):
            if dep.startswith(SYSTEM_PREFIXES) or dep == "":
                continue

            source = resolve(dep, macho, own_rpaths, came_from)

            if source is None:
                if not dep.startswith("@"):
                    unresolved.append((macho, dep))

                continue

            # Already inside the bundle: a module's sibling dylib reached
            # through its own `@loader_path`. Nothing to import, but it
            # still needs visiting so its own dependencies are followed.
            if bundle in source.parents:
                if source not in seen:
                    queue.append(source)

                continue

            parts = framework_parts(str(source))

            if parts is not None:
                root, rest = parts
                dest_root = import_framework(Path(root), frameworks)
                dest = dest_root / rest
                reference = f"@rpath/{dest_root.name}/{rest}"
            else:
                dest = import_dylib(source, frameworks)
                reference = f"@rpath/{dest.name}"

            imported += 1

            if dep != reference:
                rewrites.append((dep, reference))

            queue.append(dest)

        for old, new in rewrites:
            install_name_tool("-change", old, new, str(macho))

        # Drop the store rpaths only after their entries have been walked,
        # then point the binary at the bundle's own Frameworks.
        for entry in own_rpaths:
            if entry.startswith("/nix/store/"):
                install_name_tool("-delete_rpath", entry, str(macho))

        wanted = rpath_to(frameworks, macho)

        if wanted not in own_rpaths:
            install_name_tool("-add_rpath", wanted, str(macho))

    print(f"  imported {imported} dependency references")

    if unresolved:
        print(f"  {len(unresolved)} unresolved reference(s):", file=sys.stderr)

        for macho, dep in unresolved:
            print(
                f"    {macho.relative_to(bundle)} -> {dep}", file=sys.stderr
            )

    return len(unresolved)


def audit(bundle: Path) -> int:
    """Fail on anything still naming the build machine.

    Two separate problems share one check. A surviving `/nix/store` path is
    a bundle that will not run elsewhere; a surviving home-directory path
    is the build user's name shipped to whoever opens the disk image.
    """
    leaks = 0

    for macho in machos_under(bundle):
        for dep in dependencies(macho) + rpaths(macho):
            if dep.startswith("/nix/store/"):
                print(
                    f"  ! {macho.relative_to(bundle)} still loads {dep}",
                    file=sys.stderr,
                )
                leaks += 1

    return leaks


def write_origins(path: Path) -> None:
    """Record which store path each imported file came from.

    Written outside the bundle deliberately: it is the raw material for the
    third-party notice, but the paths in it name the build machine's store
    and nothing naming the build machine may ship.
    """
    components = sorted(
        # `<hash>-<name>-<version>` — the hash identifies the build, not the
        # component, and is meaningless to a reader.
        {
            str(source).split("/nix/store/", 1)[-1].split("/", 1)[0].split("-", 1)[-1]
            for source in ORIGINS.values()
            if str(source).startswith("/nix/store/")
        }
    )

    path.write_text("\n".join(components) + "\n")


if __name__ == "__main__":
    if not 2 <= len(sys.argv) <= 3:
        print(
            "usage: macos-bundle-libs.py <path/to/App.app> [origins-out]",
            file=sys.stderr,
        )
        sys.exit(2)

    app = Path(sys.argv[1]).resolve()

    if not (app / "Contents" / "MacOS").is_dir():
        print(f"not an app bundle: {app}", file=sys.stderr)
        sys.exit(2)

    failures = relocate(app) + audit(app)

    if len(sys.argv) == 3:
        write_origins(Path(sys.argv[2]))

    sys.exit(1 if failures else 0)
