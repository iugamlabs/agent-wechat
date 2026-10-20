
#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = [
#     "typer>=0.20,<1",
#     "rich>=14,<15",
# ]
# ///

"""Build agent-wechat Docker images locally."""

import platform
import shlex
import shutil
import subprocess
from contextlib import contextmanager
from enum import Enum
from pathlib import Path
from typing import Annotated, Iterator

import typer
from rich.console import Console
from rich.table import Table


console = Console()

ROOT_DIR = Path(__file__).resolve().parent.parent
DOCKER_DIR = ROOT_DIR / "docker"
DOCKERFILE = DOCKER_DIR / "Dockerfile"

RUST_SOURCE = ROOT_DIR / "packages" / "agent-server-rust"
RUST_CONTEXT = DOCKER_DIR / "agent-server-rust"


class Arch(str, Enum):
    amd64 = "amd64"
    arm64 = "arm64"
    both = "both"


def detect_arch() -> Arch:
    """Detect the host CPU architecture."""
    machine = platform.machine().lower()

    match machine:
        case "x86_64" | "amd64":
            return Arch.amd64
        case "aarch64" | "arm64":
            return Arch.arm64
        case _:
            raise ValueError(
                f"Unknown host architecture: {machine}. "
                "Specify --arch explicitly."
            )


def validate_environment() -> None:
    """Validate required files and Docker installation."""
    required = [
        DOCKERFILE,
        RUST_SOURCE / "Cargo.toml",
        RUST_SOURCE / "src",
        RUST_SOURCE / "migrations",
    ]

    for path in required:
        if not path.exists():
            raise FileNotFoundError(f"Required path missing: {path}")

    if shutil.which("docker") is None:
        raise RuntimeError("Docker CLI not found in PATH")


@contextmanager
def prepare_build_context() -> Iterator[None]:
    """Stage Rust source into the Docker build context."""
    console.print("[cyan]Preparing build context...[/cyan]")

    # Never overwrite an existing directory silently.
    if RUST_CONTEXT.exists() or RUST_CONTEXT.is_symlink():
        raise FileExistsError(
            f"Build context already exists: {RUST_CONTEXT}\n"
            "Inspect and remove it before retrying."
        )

    try:
        RUST_CONTEXT.mkdir()

        shutil.copy2(
            RUST_SOURCE / "Cargo.toml",
            RUST_CONTEXT / "Cargo.toml",
        )

        cargo_lock = RUST_SOURCE / "Cargo.lock"
        if cargo_lock.is_file():
            shutil.copy2(cargo_lock, RUST_CONTEXT / "Cargo.lock")

        for folder in ("src", "migrations"):
            shutil.copytree(
                RUST_SOURCE / folder,
                RUST_CONTEXT / folder,
            )

        yield

    finally:
        if RUST_CONTEXT.is_dir():
            console.print("[dim]Cleaning build context...[/dim]")
            shutil.rmtree(RUST_CONTEXT)


def build_command(
    arch: Arch,
    mode: str,
    no_cache: bool,
) -> list[str]:
    """Construct a Docker Buildx command."""
    command = ["docker", "buildx", "build"]

    if no_cache:
        command.append("--no-cache")

    command.extend([
        "--platform", f"linux/{arch.value}",
        "--build-arg", f"BUILD_MODE={mode}",
        "-t", f"agent-wechat:{arch.value}",
        "--load",
        "-f", str(DOCKERFILE),
        str(DOCKER_DIR),
    ])

    return command


def main(
    arch: Annotated[
        Arch | None,
        typer.Option("--arch", "-a", help="Target architecture"),
    ] = None,
    no_cache: Annotated[
        bool,
        typer.Option("--no-cache", help="Disable Docker build cache"),
    ] = False,
    debug: Annotated[
        bool,
        typer.Option("--debug", help="Use debug build mode"),
    ] = False,
    dry_run: Annotated[
        bool,
        typer.Option("--dry-run", help="Preview commands without building"),
    ] = False,
) -> None:
    """Build local Docker images for agent-wechat."""

    try:
        selected_arch = arch or detect_arch()
    except ValueError as exc:
        console.print(f"[red]{exc}[/red]")
        raise typer.Exit(2) from exc

    targets = (
        [Arch.amd64, Arch.arm64]
        if selected_arch == Arch.both
        else [selected_arch]
    )
    mode = "debug" if debug else "release"

    # Display configuration
    table = Table(title="Agent WeChat Docker Build")
    table.add_column("Option", style="cyan")
    table.add_column("Value", style="green")

    table.add_row("Architecture", selected_arch.value)
    table.add_row("Build mode", mode)
    table.add_row("Docker cache", "Disabled" if no_cache else "Enabled")
    table.add_row("Dry run", str(dry_run))

    console.print(table)

    commands = [
        build_command(target, mode, no_cache)
        for target in targets
    ]

    if dry_run:
        for command in commands:
            console.print(f"\n[cyan]$ {shlex.join(command)}[/cyan]")
        return

    try:
        validate_environment()

        with prepare_build_context():
            for target, command in zip(targets, commands):
                console.rule(f"Building linux/{target.value}")
                console.print(
                    f"[dim]Image: agent-wechat:{target.value}[/dim]"
                )
                subprocess.run(command, check=True)

    except subprocess.CalledProcessError as exc:
        console.print(
            f"\n[bold red]Docker build failed "
            f"(exit code {exc.returncode})[/bold red]"
        )
        raise typer.Exit(
            exc.returncode if exc.returncode > 0 else 1
        ) from exc

    except (OSError, RuntimeError) as exc:
        console.print(f"\n[bold red]Error: {exc}[/bold red]")
        raise typer.Exit(1) from exc

    except KeyboardInterrupt:
        console.print("\n[yellow]Build interrupted.[/yellow]")
        raise typer.Exit(130) from None

    console.print("\n[bold green]Build completed![/bold green]")
    for target in targets:
        console.print(f"  ✓ agent-wechat:{target.value}")


if __name__ == "__main__":
    typer.run(main)
