
#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = [
#     "typer>=0.20,<1",
#     "rich>=14,<15",
# ]
# ///

"""Publish agent-wechat Docker images to Tencent CCR."""

import os
import re
import shlex
import shutil
import subprocess
from enum import Enum
from pathlib import Path
from typing import Annotated

import typer
from rich.console import Console
from rich.markup import escape
from rich.table import Table


console = Console()

ROOT_DIR = Path(__file__).resolve().parent.parent
BUILD_SCRIPT = ROOT_DIR / "scripts" / "build-images-local.py"

DEFAULT_REPO = "ccr.ccs.tencentyun.com/deali/agent-wechat"
SOURCE_PATHS = ["docker", "packages/agent-server-rust"]


class Arch(str, Enum):
    amd64 = "amd64"
    arm64 = "arm64"


def git(*args: str) -> str:
    """Run Git and return stdout."""
    result = subprocess.run(
        ["git", "-C", str(ROOT_DIR), *args],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        raise RuntimeError(
            result.stderr.strip() or "Git command failed"
        )
    return result.stdout.strip()


def run(*args: str, env: dict | None = None) -> None:
    """Run a command, streaming its output."""
    console.print(f"[cyan]$[/cyan] {escape(shlex.join(args))}")
    subprocess.run(
        args,
        cwd=ROOT_DIR,
        env=env,
        check=True,
    )


def resolve_tag(tag: str | None) -> tuple[str, str]:
    """Resolve version and verify its Git tag."""
    if tag is None:
        tags = git(
            "tag", "--list", "v*-iugam.*",
            "--sort=-v:refname",
        ).splitlines()

        if not tags:
            raise RuntimeError(
                "No v*-iugam.* Git tag found. Pass --tag."
            )
        tag = tags[0]

    version = tag.removeprefix("v")

    # Docker tag validation
    if not re.fullmatch(
        r"[A-Za-z0-9_][A-Za-z0-9_.-]{0,127}",
        version,
    ):
        raise RuntimeError(f"Invalid version: {version}")

    git_tag = f"v{version}"

    try:
        revision = git(
            "rev-parse", "--verify",
            f"refs/tags/{git_tag}^{{commit}}",
        )
    except RuntimeError as exc:
        raise RuntimeError(
            f"Git tag {git_tag} does not exist "
            "or does not point to a commit."
        ) from exc

    return version, revision


def verify_sources(version: str) -> None:
    """Require Docker inputs to match the selected Git tag."""
    git_tag = f"v{version}"

    dirty = git(
        "status", "--porcelain",
        "--untracked-files=all",
        "--", *SOURCE_PATHS,
    )

    diff = git(
        "diff", "--stat", git_tag,
        "--", *SOURCE_PATHS,
    )

    if dirty or diff:
        console.print(
            f"[bold red]Build inputs differ from {git_tag}[/bold red]"
        )
        if dirty:
            console.print(escape(dirty))
        if diff:
            console.print(escape(diff))

        raise RuntimeError(
            "Commit changes and create a new iugam tag "
            "before publishing."
        )


def verify_unpublished(ref: str) -> None:
    """Refuse to overwrite an existing remote version tag."""
    console.print(f"\n[cyan]Checking remote:[/cyan] {ref}")

    result = subprocess.run(
        ["docker", "buildx", "imagetools", "inspect", ref],
        capture_output=True,
        text=True,
    )

    if result.returncode == 0:
        raise RuntimeError(
            f"Remote image already exists: {ref}"
        )

    output = (result.stdout + result.stderr).lower()

    # Only known missing-manifest responses permit publishing.
    not_found = (
        "not found",
        "manifest unknown",
        "manifest_unknown",
        "name unknown",
        "name_unknown",
    )

    # Authentication or network failures must not be
    # interpreted as an unpublished tag.
    fatal_errors = (
        "unauthorized",
        "denied",
        "forbidden",
        "authentication",
        "permission",
        "may require",
    )

    if (
        not any(error in output for error in not_found)
        or any(error in output for error in fatal_errors)
    ):
        raise RuntimeError(
            f"Cannot confirm remote tag is unpublished:\n"
            f"{result.stdout}{result.stderr}"
        )

    console.print("[green]Remote version is available.[/green]")


def publish(
    version: str,
    arch: Arch,
    repo: str,
    push_latest: bool,
    no_cache: bool,
) -> None:
    """Build, tag and push the Docker image."""
    local_image = f"agent-wechat:{arch.value}"
    remote_image = f"{repo}:{version}"

    env = os.environ.copy()
    env["BUILDKIT_PROGRESS"] = "plain"

    # Build using the shared Python build script.
    build_args = [
        "uv", "run", "--script",
        str(BUILD_SCRIPT),
        "--arch", arch.value,
    ]

    if no_cache:
        build_args.append("--no-cache")

    console.rule("Build")
    run(*build_args, env=env)

    console.rule("Push version")
    run("docker", "tag", local_image, remote_image)
    run("docker", "push", remote_image)

    if push_latest:
        latest = f"{repo}:latest"

        console.rule("Push latest")
        run("docker", "tag", local_image, latest)
        run("docker", "push", latest)

    console.rule("Verify")
    run("docker", "buildx", "imagetools", "inspect", remote_image)

    console.print("\n[bold green]Publish completed![/bold green]")
    console.print(f"  ✓ {remote_image}")

    if push_latest:
        console.print(f"  ✓ {repo}:latest")


def main(
    tag: Annotated[
        str | None,
        typer.Option("--tag", help="Version (default: latest local iugam tag)"),
    ] = None,
    arch: Annotated[
        Arch,
        typer.Option("--arch", "-a", help="Target architecture"),
    ] = Arch.amd64,
    repo: Annotated[
        str,
        typer.Option("--repo", envvar="IMAGE_REPO", help="Image repository"),
    ] = DEFAULT_REPO,
    no_latest: Annotated[
        bool,
        typer.Option("--no-latest", help="Do not update latest"),
    ] = False,
    no_cache: Annotated[
        bool,
        typer.Option("--no-cache", help="Disable build cache"),
    ] = False,
    dry_run: Annotated[
        bool,
        typer.Option("--dry-run", help="Validate and preview without publishing"),
    ] = False,
) -> None:
    """Build and publish an agent-wechat image to Tencent CCR."""
    try:
        for executable in ("git", "docker", "uv"):
            if shutil.which(executable) is None:
                raise RuntimeError(f"{executable} not found in PATH")

        if not BUILD_SCRIPT.is_file():
            raise RuntimeError(f"Build script missing: {BUILD_SCRIPT}")

        version, revision = resolve_tag(tag)
        verify_sources(version)

        repo = repo.rstrip("/")
        if not repo:
            raise RuntimeError("Image repository cannot be empty")

        ref = f"{repo}:{version}"

        table = Table(title="Agent WeChat Publisher")
        table.add_column("Option", style="cyan")
        table.add_column("Value", style="green")
        table.add_row("Git tag", f"v{version}")
        table.add_row("Revision", revision[:12])
        table.add_row("Architecture", arch.value)
        table.add_row("Repository", repo)
        table.add_row("Push latest", str(not no_latest))
        table.add_row("Build cache", "Disabled" if no_cache else "Enabled")
        table.add_row("Dry run", str(dry_run))

        console.print(table)

        if dry_run:
            console.print("\n[yellow]Dry run: no build or push.[/yellow]")
            console.print(
                "[dim]Remote tag availability was not checked.[/dim]"
            )
            console.print(f"Would publish: {ref}")
            if not no_latest:
                console.print(f"Would update: {repo}:latest")
            return

        verify_unpublished(ref)

        publish(
            version=version,
            arch=arch,
            repo=repo,
            push_latest=not no_latest,
            no_cache=no_cache,
        )

    except subprocess.CalledProcessError as exc:
        console.print(
            f"\n[bold red]Command failed "
            f"(exit code {exc.returncode})[/bold red]"
        )
        raise typer.Exit(
            exc.returncode if exc.returncode > 0 else 1
        ) from exc

    except (RuntimeError, OSError) as exc:
        console.print(
            f"\n[bold red]Error:[/bold red] {escape(str(exc))}"
        )
        raise typer.Exit(1) from exc

    except KeyboardInterrupt:
        console.print("\n[yellow]Publishing interrupted.[/yellow]")
        raise typer.Exit(130) from None


if __name__ == "__main__":
    typer.run(main)
