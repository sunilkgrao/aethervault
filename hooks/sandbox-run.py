#!/usr/bin/env python3
"""Sandboxed Python code execution for AetherVault.

Two execution tiers:
  --monty "expr"     Fast tier: Monty (Rust interpreter, microseconds, limited stdlib)
  -c "code"          Full tier: subprocess sandbox (full Python, 30s timeout)
  /path/to/file.py   Full tier: run a script file

Examples:
  python3 sandbox-run.py --monty "2 ** 100"
  python3 sandbox-run.py --monty "def fib(n): return n if n<=1 else fib(n-1)+fib(n-2)\nfib(20)"
  python3 sandbox-run.py -c "import math; print(math.pi)"
  python3 sandbox-run.py /tmp/my_script.py
  python3 sandbox-run.py --timeout 60 /tmp/heavy_script.py
"""
import sys, subprocess, tempfile, os, json, io, contextlib

MAX_TIMEOUT = 30
MAX_OUTPUT = 10000

# Try importing Monty
try:
    import pydantic_monty
    HAS_MONTY = True
except ImportError:
    HAS_MONTY = False


def run_monty(code):
    """Run code via Monty (Rust interpreter) — microsecond execution."""
    if not HAS_MONTY:
        return {"exit_code": -1, "status": "error",
                "error": "pydantic-monty not installed"}
    try:
        # Capture stdout from print() calls
        stdout_capture = io.StringIO()
        with contextlib.redirect_stdout(stdout_capture):
            m = pydantic_monty.Monty(code)
            result = m.run()

        stdout_text = stdout_capture.getvalue()
        output = {"exit_code": 0, "status": "success", "engine": "monty"}

        if stdout_text:
            output["stdout"] = stdout_text[:MAX_OUTPUT]
        if result is not None:
            output["result"] = repr(result)[:MAX_OUTPUT]
        if not stdout_text and result is None:
            output["stdout"] = ""

        return output
    except Exception as e:
        return {"exit_code": 1, "status": "error", "engine": "monty",
                "error": str(e)[:MAX_OUTPUT]}


def run_subprocess(code, timeout=MAX_TIMEOUT):
    """Run Python code in a restricted subprocess."""
    with tempfile.NamedTemporaryFile(
        mode="w", suffix=".py", delete=False, dir="/tmp", prefix="sandbox_"
    ) as f:
        f.write(code)
        f.flush()
        script_path = f.name

    try:
        env = {
            "PATH": "/usr/local/bin:/usr/bin:/bin",
            "HOME": "/tmp",
            "PYTHONDONTWRITEBYTECODE": "1",
            "PYTHONUNBUFFERED": "1",
            "LANG": "C.UTF-8",
        }

        result = subprocess.run(
            ["python3", "-u", script_path],
            capture_output=True,
            text=True,
            timeout=timeout,
            cwd="/tmp",
            env=env,
        )

        output = {
            "exit_code": result.returncode,
            "stdout": result.stdout[:MAX_OUTPUT],
            "engine": "subprocess",
        }
        if result.stderr:
            output["stderr"] = result.stderr[:MAX_OUTPUT]
        output["status"] = "success" if result.returncode == 0 else "error"
        return output

    except subprocess.TimeoutExpired:
        return {"exit_code": -1, "status": "timeout", "engine": "subprocess",
                "error": f"Execution timed out after {timeout}s"}
    except Exception as e:
        return {"exit_code": -1, "status": "error", "engine": "subprocess",
                "error": str(e)}
    finally:
        try:
            os.unlink(script_path)
        except OSError:
            pass


def main():
    timeout = MAX_TIMEOUT
    code = None
    use_monty = False

    args = sys.argv[1:]
    i = 0
    while i < len(args):
        if args[i] == "--timeout" and i + 1 < len(args):
            timeout = int(args[i + 1])
            i += 2
        elif args[i] == "--monty" and i + 1 < len(args):
            code = args[i + 1]
            use_monty = True
            i += 2
        elif args[i] == "--stdin":
            code = sys.stdin.read()
            i += 1
        elif args[i] == "-c" and i + 1 < len(args):
            code = args[i + 1]
            i += 2
        elif not args[i].startswith("-"):
            try:
                with open(args[i]) as f:
                    code = f.read()
            except FileNotFoundError:
                print(json.dumps({"exit_code": -1, "status": "error",
                                  "error": f"File not found: {args[i]}"}))
                sys.exit(1)
            i += 1
        else:
            i += 1

    if code is None:
        print(__doc__)
        sys.exit(1)

    if use_monty:
        result = run_monty(code)
    else:
        result = run_subprocess(code, timeout)

    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
