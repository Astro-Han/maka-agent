"""Behavior contract for the shared command-scope teardown.

``cleanup_process_scope`` only ever runs from an adapter's ``finally`` while the
agent's own exception is in flight (claude_code_agent.py, codex_agent.py,
kimi_code_agent.py all guard it with ``if abnormal_exit``). An exception raised
there replaces the exception being propagated, so a teardown that raises rewrites
the trial's cause: an ``AgentTimeoutError`` reached Harbor as a
``NonZeroAgentExitCodeError``, which the runner reads as an infrastructure
failure instead of a graded timeout.

Run directly: ``python3 packages/headless/harbor/tests/test_process_scope.py``
"""

from __future__ import annotations

import asyncio
import shutil
import subprocess
import sys
import time
import uuid
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from process_scope import (  # noqa: E402
    COMMAND_SCOPE_ROOT,
    cleanup_process_scope,
    scoped_command,
    scoped_process_cleanup_command,
)


class _Logger:
    def __init__(self) -> None:
        self.warnings: list[str] = []

    def warning(self, message: str, *args: object) -> None:
        self.warnings.append(message % args)


class _Agent:
    def __init__(self, fail: bool) -> None:
        self.logger = _Logger()
        self.commands: list[str] = []
        self._fail = fail

    async def exec_as_agent(self, environment: object, command: str) -> None:
        self.commands.append(command)
        if self._fail:
            raise RuntimeError("Command failed (exit -9)")


def test_a_failing_teardown_never_replaces_the_agent_failure() -> None:
    agent = _Agent(fail=True)

    async def run() -> None:
        try:
            raise TimeoutError("agent deadline")
        finally:
            await cleanup_process_scope(agent, object(), "scope-1")

    try:
        asyncio.run(run())
    except TimeoutError:
        pass
    else:
        raise AssertionError("the agent's own failure did not survive teardown")
    assert len(agent.commands) == 2, agent.commands
    assert len(agent.logger.warnings) == 2, agent.logger.warnings


def test_kill_still_runs_after_term_fails() -> None:
    agent = _Agent(fail=True)
    asyncio.run(cleanup_process_scope(agent, object(), "scope-2"))
    signals = ["KILL" if "kill -KILL" in c else "TERM" for c in agent.commands]
    assert signals == ["TERM", "KILL"], signals


def test_a_broken_logger_never_replaces_the_agent_failure() -> None:
    class _RaisingLogger(_Logger):
        def warning(self, message: str, *args: object) -> None:
            raise RuntimeError("logging handler is down")

    missing = _Agent(fail=True)
    del missing.logger
    raising = _Agent(fail=True)
    raising.logger = _RaisingLogger()

    for agent in (missing, raising):

        async def run(agent: _Agent = agent) -> None:
            try:
                raise TimeoutError("agent deadline")
            finally:
                await cleanup_process_scope(agent, object(), "scope-4")

        try:
            asyncio.run(run())
        except TimeoutError:
            continue
        raise AssertionError("reporting the teardown failure became one")


def test_cancellation_still_propagates() -> None:
    class _Cancelling(_Agent):
        async def exec_as_agent(self, environment: object, command: str) -> None:
            self.commands.append(command)
            raise asyncio.CancelledError

    agent = _Cancelling(fail=False)
    try:
        asyncio.run(cleanup_process_scope(agent, object(), "scope-3"))
    except asyncio.CancelledError:
        pass
    else:
        raise AssertionError("teardown swallowed cancellation")
    assert len(agent.commands) == 1, agent.commands


def _holders(stdout_path: Path) -> list[str]:
    """The replay processes still holding the pipe open. Draining the pipe would
    unblock the replay and hide the defect, so ask the process table instead.

    Matching the path alone would also match the wrapper, whose `bash -c` script
    text contains that path: the pre-cleanup guard below would then pass with no
    replay alive at all, which is a false PASS in the one direction that matters.
    So require the command itself to be the replay.
    """
    listing = subprocess.run(  # noqa: S603 - fixed argv, no shell interpolation
        ["ps", "-A", "-o", "pid=,command="],
        check=False,
        timeout=30,
        capture_output=True,
        text=True,
    ).stdout.splitlines()
    marker = f"cat -- {stdout_path}"
    return [
        line.strip()
        for line in listing
        if line.strip().split(maxsplit=1)[1:2] == [marker]
    ]


def test_cleanup_reaps_the_output_replay_so_the_reader_reaches_eof() -> None:
    """The wrapper captures the command's output to a file and replays it after
    the command exits, so that a descendant outliving the command cannot hold the
    exec's stdout open. The replay itself is the hole: when the caller stops
    reading — which is exactly what an agent-phase timeout does — it blocks in
    ``pipe_write`` forever, and teardown has no handle on it. It is not the
    wrapper PID and it carries no scope marker, so whether anything reaps it
    comes down to which process group the shell happened to leave it in. On a
    host with no ``/proc`` the marker sweep is a no-op and nothing does, which is
    what this test pins. A stranded replay means the exec never sees EOF, so the
    cell hangs and the verifier never runs.
    """
    if shutil.which("bash") is None or shutil.which("ps") is None:
        return

    scope = f"test-scope-{uuid.uuid4().hex}"
    command_id = "replay"
    stdout_path = Path(COMMAND_SCOPE_ROOT) / scope / f"{command_id}.stdout"
    # Comfortably past a 64 KiB pipe buffer, so the replay blocks partway
    # through rather than finishing before anyone can observe it.
    payload = "yes 0123456789abcdef | head -c 400000"

    wrapper = subprocess.Popen(  # noqa: S603 - fixed argv, no shell interpolation
        ["bash", "-c", scoped_command(payload, scope, command_id)],
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
    )
    try:
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            if stdout_path.exists() and stdout_path.stat().st_size >= 400000:
                break
            time.sleep(0.1)
        else:
            raise AssertionError("the command never finished writing its output")
        # The wrapper starts the replay once the command exits; give it that
        # step before asserting on what teardown can still reach.
        time.sleep(1)
        assert _holders(stdout_path), (
            "the replay never blocked, so this test would pass no matter what "
            "teardown reaps"
        )

        for signal in ("TERM", "KILL"):
            subprocess.run(  # noqa: S603 - fixed argv, no shell interpolation
                ["bash", "-c", scoped_process_cleanup_command(scope, signal)],
                check=False,
                timeout=30,
                capture_output=True,
            )

        survivors = _holders(stdout_path)
        assert not survivors, (
            f"teardown left {len(survivors)} process(es) holding the exec's "
            "pipe; that is the hang that strands a cell with no verifier"
        )
    finally:
        wrapper.kill()
        wrapper.wait(timeout=10)
        shutil.rmtree(Path(COMMAND_SCOPE_ROOT) / scope, ignore_errors=True)


def _main() -> int:
    tests = [value for name, value in sorted(globals().items()) if name.startswith("test_")]
    failures = 0
    for test in tests:
        try:
            test()
        except Exception as error:  # noqa: BLE001 - standalone runner reports all
            failures += 1
            print(f"FAIL {test.__name__}: {error!r}")
        else:
            print(f"PASS {test.__name__}")
    print(f"\n{len(tests) - failures} passed, {failures} failed")
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(_main())
