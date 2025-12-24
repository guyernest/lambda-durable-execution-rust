# /// script
# requires-python = ">=3.11"
# dependencies = [
#     "click>=8.0",
#     "boto3>=1.35",
#     "tqdm>=4.66",
# ]
# ///
"""
Durable execution examples validator using boto3 and Click.

Usage:
    uv run examples/scripts/validate.py
    uv run examples/scripts/validate.py --mermaid
    uv run examples/scripts/validate.py --mermaid
    uv run examples/scripts/validate.py --example HelloWorldExampleFunctionArn --mermaid

"""
from __future__ import annotations

import dataclasses
import json
import time
from pathlib import Path
from typing import Any, Iterable

import boto3
import click
from tqdm import tqdm


# -----------------------------------------------------------------------------
# Output Styling
# -----------------------------------------------------------------------------

# Box drawing characters
BOX_TL = "+"
BOX_TR = "+"
BOX_BL = "+"
BOX_BR = "+"
BOX_H = "-"
BOX_V = "|"

# Status symbols
SYM_OK = "[OK]"
SYM_FAIL = "[FAIL]"
SYM_WAIT = "[..]"
SYM_ARROW = "==>"


def styled(text: str, **kwargs: Any) -> str:
    """Apply click styling to text."""
    return click.style(text, **kwargs)


def print_header(title: str, width: int = 60) -> None:
    """Print a styled header box."""
    padding = (width - len(title) - 2) // 2
    click.echo()
    click.echo(styled(BOX_TL + BOX_H * (width - 2) + BOX_TR, fg="cyan"))
    click.echo(styled(BOX_V, fg="cyan") + " " * padding + styled(title, fg="cyan", bold=True) + " " * (width - padding - len(title) - 2) + styled(BOX_V, fg="cyan"))
    click.echo(styled(BOX_BL + BOX_H * (width - 2) + BOX_BR, fg="cyan"))


def print_section(title: str) -> None:
    """Print a section header."""
    click.echo()
    click.echo(styled(f"{SYM_ARROW} {title}", fg="yellow", bold=True))
    click.echo(styled("-" * 50, fg="yellow", dim=True))


def print_status(name: str, status: str, detail: str = "") -> None:
    """Print a status line with colored indicator."""
    if status == "SUCCEEDED":
        symbol = styled(SYM_OK, fg="green", bold=True)
        status_text = styled(status, fg="green")
    elif status in ("FAILED", "TIMED_OUT", "STOPPED"):
        symbol = styled(SYM_FAIL, fg="red", bold=True)
        status_text = styled(status, fg="red")
    else:
        symbol = styled(SYM_WAIT, fg="yellow")
        status_text = styled(status, fg="yellow")

    line = f"  {symbol} {name}: {status_text}"
    if detail:
        line += styled(f" ({detail})", dim=True)
    click.echo(line)


def print_summary_table(results: list[dict[str, Any]]) -> None:
    """Print a summary table of results."""
    total = len(results)
    succeeded = sum(1 for r in results if r["status"] == "SUCCEEDED")
    failed = total - succeeded

    inner_width = 40

    click.echo()
    click.echo(styled(BOX_TL + BOX_H * inner_width + BOX_TR, fg="cyan"))
    click.echo(
        styled(BOX_V, fg="cyan")
        + styled("  VALIDATION SUMMARY", bold=True).ljust(inner_width)
        + styled(BOX_V, fg="cyan")
    )
    click.echo(styled(BOX_V + BOX_H * inner_width + BOX_V, fg="cyan"))

    total_line = f"  Total examples: {total}"
    click.echo(styled(BOX_V, fg="cyan") + total_line.ljust(inner_width) + styled(BOX_V, fg="cyan"))

    passed_line = f"  Passed: {succeeded}"
    click.echo(
        styled(BOX_V, fg="cyan")
        + styled(passed_line, fg="green").ljust(inner_width)
        + styled(BOX_V, fg="cyan")
    )

    failed_line = f"  Failed: {failed}"
    failed_styled = styled(failed_line, fg="red") if failed > 0 else failed_line
    click.echo(
        styled(BOX_V, fg="cyan")
        + failed_styled.ljust(inner_width)
        + styled(BOX_V, fg="cyan")
    )

    click.echo(styled(BOX_BL + BOX_H * inner_width + BOX_BR, fg="cyan"))

    if failed == 0:
        click.echo()
        click.echo(styled("  All validations passed!", fg="green", bold=True))
    else:
        click.echo()
        click.echo(styled("  Some validations failed. Check details above.", fg="red", bold=True))


# -----------------------------------------------------------------------------
# AWS API Functions (boto3)
# -----------------------------------------------------------------------------


def get_stack_outputs(cfn_client: Any, stack_name: str) -> dict[str, str]:
    """Get CloudFormation stack outputs as a dictionary."""
    resp = cfn_client.describe_stacks(StackName=stack_name)
    outputs = resp["Stacks"][0].get("Outputs", [])
    return {o["OutputKey"]: o["OutputValue"] for o in outputs}


def invoke_lambda(
    lambda_client: Any,
    function_arn: str,
    payload: Any,
    qualifier: str = "$LATEST",
) -> str:
    """Invoke a Lambda function asynchronously and return the DurableExecutionArn."""
    resp = lambda_client.invoke(
        FunctionName=function_arn,
        Qualifier=qualifier,
        InvocationType="Event",
        Payload=json.dumps(payload).encode("utf-8"),
    )
    durable_arn = resp.get("DurableExecutionArn")
    if not isinstance(durable_arn, str) or not durable_arn:
        raise RuntimeError(
            f"Invoke did not return DurableExecutionArn for {function_arn}: {resp}"
        )
    return durable_arn


def get_durable_execution(
    lambda_client: Any,
    durable_execution_arn: str,
    max_retries: int = 10,
    retry_delay: float = 1.0,
) -> dict[str, Any]:
    """Get durable execution status with retry logic."""
    last_err: Exception | None = None
    for _ in range(max_retries):
        try:
            return lambda_client.get_durable_execution(
                DurableExecutionArn=durable_execution_arn
            )
        except Exception as e:
            last_err = e
            time.sleep(retry_delay)
    raise last_err or RuntimeError("get_durable_execution failed")


def get_durable_execution_history(
    lambda_client: Any,
    durable_execution_arn: str,
    max_retries: int = 10,
    retry_delay: float = 1.0,
) -> dict[str, Any]:
    """Get durable execution history with retry logic."""
    last_err: Exception | None = None
    for _ in range(max_retries):
        try:
            return lambda_client.get_durable_execution_history(
                DurableExecutionArn=durable_execution_arn,
                IncludeExecutionData=True,
            )
        except Exception as e:
            last_err = e
            time.sleep(retry_delay)
    raise last_err or RuntimeError("get_durable_execution_history failed")


def send_callback_success(
    lambda_client: Any,
    callback_id: str,
    result_bytes: bytes,
) -> None:
    """Send a callback success response."""
    lambda_client.send_durable_execution_callback_success(
        CallbackId=callback_id,
        Result=result_bytes,
    )


# -----------------------------------------------------------------------------
# Helper Functions
# -----------------------------------------------------------------------------


def find_values_by_key(obj: Any, key: str) -> Iterable[Any]:
    """Recursively find all values for a given key in a nested structure."""
    if isinstance(obj, dict):
        for k, v in obj.items():
            if k == key:
                yield v
            yield from find_values_by_key(v, key)
    elif isinstance(obj, list):
        for v in obj:
            yield from find_values_by_key(v, key)


def extract_callback_ids(history: dict[str, Any]) -> list[str]:
    """Extract unique callback IDs from execution history, preserving order."""
    ids: list[str] = []
    for v in find_values_by_key(history, "CallbackId"):
        if isinstance(v, str) and v:
            ids.append(v)
    # De-duplicate while preserving order
    seen: set[str] = set()
    out: list[str] = []
    for cid in ids:
        if cid not in seen:
            seen.add(cid)
            out.append(cid)
    return out


def errors_from_history(history: dict[str, Any]) -> list[str]:
    """Extract error messages from execution history."""
    out: list[str] = []
    for event in history.get("Events", []) or []:
        details = event.get("InvocationCompletedDetails")
        if not isinstance(details, dict):
            continue
        err = details.get("Error")
        if not isinstance(err, dict):
            continue
        payload = err.get("Payload")
        if payload is None:
            out.append("InvocationCompletedDetails.Error present (no payload)")
        else:
            out.append(json.dumps(payload, sort_keys=True))
    return out


# -----------------------------------------------------------------------------
# Mermaid Diagram Generation
# -----------------------------------------------------------------------------


def generate_mermaid_diagram(history: dict[str, Any]) -> str:
    """Generate a Mermaid flowchart from execution history."""
    events = history.get("Events", [])
    if not events:
        return "flowchart TD\n    empty[No events]"

    lines: list[str] = ["flowchart TD"]

    # Track contexts and their children
    contexts: dict[str, dict[str, Any]] = {}  # id -> context info
    # Track events by their ID for lookup
    events_by_ref: dict[str, dict[str, Any]] = {}
    # Track event order for sequencing
    event_order: list[dict[str, Any]] = []

    # First pass: collect all events and build context map
    for event in events:
        event_type = event.get("EventType", "")
        op_id = event.get("Id", str(event.get("EventId", "")))
        event_num = int(event.get("EventId", 0) or 0)
        name = event.get("Name", "")
        sub_type = event.get("SubType", "")
        parent_id = event.get("ParentId")

        # Skip InvocationCompleted events (internal)
        if event_type == "InvocationCompleted":
            continue

        ref = f"{op_id}:{event_type}:{event_num}"
        event_info = {
            "id": op_id,
            "name": name,
            "type": event_type,
            "sub_type": sub_type,
            "parent_id": parent_id,
            "event_id": event_num,
        }
        events_by_ref[ref] = event_info

        # Track context (parallel, branches, etc.)
        if event_type == "ContextStarted":
            contexts[op_id] = {
                "name": name,
                "sub_type": sub_type,
                "children": [],
                "parent_id": parent_id,
                "event_id": event_num,
            }
            # Register with parent context
            if parent_id and parent_id in contexts:
                contexts[parent_id]["children"].append(op_id)
            elif sub_type == "ParallelBranch":
                # Infer parent from name convention: "parent_name-branch-N"
                # Find the most recent Parallel context that matches
                if "-branch-" in name:
                    parent_name = name.rsplit("-branch-", 1)[0]
                    for ctx_id, ctx in reversed(list(contexts.items())):
                        if ctx["name"] == parent_name and ctx["sub_type"] == "Parallel":
                            ctx["children"].append(op_id)
                            contexts[op_id]["parent_id"] = ctx_id
                            break
            elif parent_id is None:
                event_order.append(event_info)

        # Track operations under their parent context
        elif event_type.startswith("Step"):
            if parent_id and parent_id in contexts:
                contexts[parent_id]["children"].append(ref)
            elif parent_id is None:
                event_order.append(event_info)

        elif event_type.startswith("Wait"):
            if parent_id and parent_id in contexts:
                contexts[parent_id]["children"].append(ref)
            elif parent_id is None:
                event_order.append(event_info)

        elif event_type.startswith("Callback"):
            if parent_id and parent_id in contexts:
                contexts[parent_id]["children"].append(ref)
            elif parent_id is None:
                event_order.append(event_info)

        elif event_type.startswith("ChainedInvoke"):
            if parent_id and parent_id in contexts:
                contexts[parent_id]["children"].append(ref)
            elif parent_id is None:
                event_order.append(event_info)

        elif event_type == "ExecutionStarted":
            event_order.insert(0, event_info)

        elif event_type in ("ExecutionSucceeded", "ExecutionFailed"):
            event_order.append(event_info)

    def _safe_id(s: str, suffix: str = "") -> str:
        """Create a safe Mermaid node ID."""
        base = s.replace("-", "_").replace("$", "_").replace(":", "_")[:16]
        return f"n_{base}{suffix}"

    def node_label(event_info: dict[str, Any]) -> str:
        name = event_info["name"] or event_info["id"][:8]
        sub_type = event_info["sub_type"]
        event_type = event_info["type"]

        if event_type == "ExecutionStarted":
            return f'([Start])'
        elif event_type == "ExecutionSucceeded":
            return f'([Success])'
        elif event_type == "ExecutionFailed":
            return f'([Failed])'
        elif "ChainedInvoke" in event_type or sub_type == "ChainedInvoke":
            return f'["invoke: {name}"]'
        elif sub_type == "Step" or "Step" in event_type:
            return f'["{name}"]'
        elif sub_type == "Wait" or "Wait" in event_type:
            return f'[/"{name}"/]'
        elif sub_type == "Callback" or "Callback" in event_type:
            return f'{{{{"{name}"}}}}'
        else:
            return f'["{name}"]'

    def node_class(event_info: dict[str, Any]) -> str:
        # Intentionally unused: we avoid emitting Mermaid class styling so the
        # diagrams render legibly on both light and dark backgrounds.
        return ""

    def render_context(ctx_id: str, indent: int = 1) -> list[str]:
        """Render a context (parallel, branch, etc.) as a subgraph."""
        ctx = contexts.get(ctx_id)
        if not ctx:
            return []

        result: list[str] = []
        ind = "    " * indent
        sub_type = ctx.get("sub_type", "")
        name = ctx.get("name") or sub_type or ctx_id[:8]

        # Determine subgraph label
        if sub_type == "ParallelBranch":
            # Extract branch number from name like "parallel_operation-branch-0"
            if "-branch-" in name:
                branch_label = "branch-" + name.split("-branch-")[-1]
            else:
                branch_label = name
        else:
            branch_label = name

        result.append(f'{ind}subgraph {_safe_id(ctx_id)}["{branch_label}"]')

        # Render children
        for child_ref in ctx.get("children", []):
            # Child can be a context ID or an event reference
            if child_ref in contexts:
                result.extend(render_context(child_ref, indent + 1))
            else:
                event_info = events_by_ref.get(child_ref)
                if event_info:
                    node_id = _safe_id(
                        event_info["id"],
                        f"_{event_info['type'][:4]}_{event_info['event_id']}",
                    )
                    result.append(f'{ind}    {node_id}{node_label(event_info)}')

        result.append(f"{ind}end")
        return result

    # Render the diagram
    rendered_ids: set[str] = set()
    flow_nodes: list[str] = []

    for event_info in event_order:
        event_type = event_info["type"]
        event_id = event_info["id"]

        if event_type == "ExecutionStarted":
            node_id = _safe_id(event_id, "_start")
            lines.append(f'    {node_id}{node_label(event_info)}')
            flow_nodes.append(node_id)
            rendered_ids.add(event_id)

        elif event_type in ("ExecutionSucceeded", "ExecutionFailed"):
            node_id = _safe_id(event_id, "_end")
            lines.append(f'    {node_id}{node_label(event_info)}')
            flow_nodes.append(node_id)

        elif event_type == "ContextStarted":
            # Render context as subgraph
            if event_id in contexts:
                lines.extend(render_context(event_id))
                flow_nodes.append(_safe_id(event_id))
                rendered_ids.add(event_id)

        else:
            # Regular node (step, wait at top level)
            node_id = _safe_id(event_id, f"_{event_type[:4]}_{event_info['event_id']}")
            lines.append(f'    {node_id}{node_label(event_info)}')
            flow_nodes.append(node_id)
            rendered_ids.add(event_id)

    # Build flow connections
    if len(flow_nodes) > 1:
        lines.append(f'    {" --> ".join(flow_nodes)}')

    return "\n".join(lines)


# -----------------------------------------------------------------------------
# Example docstring extraction
# -----------------------------------------------------------------------------


def load_example_docstring(bin_name: str) -> tuple[str, str]:
    """Load the top-level docstring from an example source file.

    Returns (title, summary) where summary is the first paragraph after the title.
    """
    source_path = Path(__file__).resolve().parents[1] / "src" / "bin" / bin_name / "main.rs"
    if not source_path.exists():
        return (bin_name, "")

    doc_lines: list[str] = []
    for line in source_path.read_text(encoding="utf-8").splitlines():
        if line.startswith("//!"):
            doc_lines.append(line[3:].lstrip())
        elif doc_lines:
            break

    # Trim leading/trailing empty lines
    while doc_lines and not doc_lines[0].strip():
        doc_lines.pop(0)
    while doc_lines and not doc_lines[-1].strip():
        doc_lines.pop()

    if not doc_lines:
        return (bin_name, "")

    title = doc_lines[0].strip()
    rest = doc_lines[1:]
    while rest and not rest[0].strip():
        rest.pop(0)

    summary_lines: list[str] = []
    for line in rest:
        if not line.strip():
            break
        summary_lines.append(line.rstrip())

    summary = "\n".join(summary_lines).strip()
    return (title, summary)


# -----------------------------------------------------------------------------
# Example Specifications
# -----------------------------------------------------------------------------


@dataclasses.dataclass(frozen=True)
class ExampleSpec:
    output_key: str
    bin_name: str
    payload: Any
    callback_result: Any | None = None
    callback_result_sequence: list[Any] | None = None


def get_example_specs() -> list[ExampleSpec]:
    """Return the list of example specifications."""
    return [
        ExampleSpec(
            "HelloWorldExampleFunctionArn",
            "hello_world",
            {"name": "World"},
        ),
        ExampleSpec(
            "CallbackExampleFunctionArn",
            "callback_example",
            {
                "request_id": "req-1",
                "description": "Approve this request",
                "approver_email": "approver@example.com",
            },
            callback_result={"approved": True, "comment": "approved"},
        ),
        ExampleSpec(
            "StepRetryExampleFunctionArn",
            "step_retry",
            {"url": "https://example.com", "max_retries": 3},
        ),
        ExampleSpec(
            "ChildContextExampleFunctionArn",
            "child_context",
            {"items": ["a", "b", "c"]},
        ),
        ExampleSpec("MapOperationsExampleFunctionArn", "map_operations", {}),
        ExampleSpec("ParallelExampleFunctionArn", "parallel", {}),
        ExampleSpec(
            "ParallelFirstSuccessfulExampleFunctionArn",
            "parallel_first_successful",
            {},
        ),
        ExampleSpec("MapWithFailureToleranceExampleFunctionArn", "map_with_failure_tolerance", {}),
        ExampleSpec("WaitForConditionExampleFunctionArn", "wait_for_condition", {}),
        ExampleSpec("MapWithCustomSerdesExampleFunctionArn", "map_with_custom_serdes", {}),
        ExampleSpec(
            "WaitForCallbackHeartbeatExampleFunctionArn",
            "wait_for_callback_heartbeat",
            {},
            callback_result="ok",
        ),
        ExampleSpec(
            "WaitForCallbackMultipleInvocationsExampleFunctionArn",
            "wait_for_callback_multiple_invocations",
            {},
            callback_result_sequence=["first", "second"],
        ),
        ExampleSpec("BlockExampleFunctionArn", "block_example", {}),
        ExampleSpec("InvokeTargetExampleFunctionArn", "invoke_target", {"value": 21}),
        ExampleSpec("InvokeCallerExampleFunctionArn", "invoke_caller", {"value": 21}),
    ]


def callback_result_bytes_for(spec: ExampleSpec, callback_index: int) -> bytes:
    """Get the callback result bytes for a given spec and callback index."""
    if spec.callback_result_sequence is not None:
        value = spec.callback_result_sequence[callback_index]
    else:
        value = spec.callback_result
    if value is None:
        raise RuntimeError("callback_result is required for callback examples")
    return json.dumps(value).encode("utf-8")


# -----------------------------------------------------------------------------
# Main Validation Logic
# -----------------------------------------------------------------------------


def run_validation(
    region: str,
    stack: str,
    out_dir: Path,
    timeout_seconds: int,
    poll_seconds: float,
    generate_mermaid: bool,
    diagrams_dir: Path,
    example_filter: tuple[str, ...],
) -> int:
    """Run the validation and return exit code (0 = success, 1 = failures)."""
    out_dir.mkdir(parents=True, exist_ok=True)
    if generate_mermaid:
        diagrams_dir.mkdir(parents=True, exist_ok=True)

    # Print header
    print_header("DURABLE EXECUTION VALIDATOR")

    # Create boto3 clients
    click.echo()
    click.echo(styled("  Initializing...", dim=True))
    cfn_client = boto3.client("cloudformation", region_name=region)
    lambda_client = boto3.client("lambda", region_name=region)

    # Get stack outputs
    click.echo(styled(f"  Loading stack: {stack}", dim=True))
    click.echo(styled(f"  Region: {region}", dim=True))
    click.echo(styled(f"  Output: {out_dir}", dim=True))
    outputs = get_stack_outputs(cfn_client, stack)

    # Filter specs if requested
    specs = get_example_specs()
    if example_filter:
        specs = [s for s in specs if s.output_key in example_filter]
        if not specs:
            click.echo(styled(f"  No examples matched filter: {example_filter}", fg="red"), err=True)
            return 1

    print_section(f"Running {len(specs)} example(s)")

    results: list[dict[str, Any]] = []

    for spec_idx, spec in enumerate(specs, 1):
        function_arn = outputs.get(spec.output_key)
        if not function_arn:
            click.echo(styled(f"  Missing stack output: {spec.output_key}", fg="red"), err=True)
            continue

        # Display example name
        example_name = spec.bin_name
        click.echo()
        click.echo(styled(f"  [{spec_idx}/{len(specs)}] ", fg="cyan", bold=True) + styled(example_name, bold=True))

        # Invoke
        click.echo(styled("      Invoking Lambda...", dim=True))
        durable_arn = invoke_lambda(lambda_client, function_arn, spec.payload)

        callback_sent: set[str] = set()
        callback_first_seen: dict[str, float] = {}
        start_time = time.time()
        status = "UNKNOWN"
        terminal = False
        all_errors: list[str] = []
        history: dict[str, Any] = {}

        # Create progress bar for polling
        max_iterations = int(timeout_seconds / poll_seconds)
        with tqdm(
            total=max_iterations,
            desc="      Waiting",
            bar_format="      {desc}: {bar:20} {elapsed} | {postfix}",
            leave=False,
            colour="cyan",
        ) as pbar:
            pbar.set_postfix_str(status)

            iteration = 0
            while time.time() - start_time < timeout_seconds:
                exec_info = get_durable_execution(lambda_client, durable_arn)
                status = exec_info.get("Status", "UNKNOWN")
                pbar.set_postfix_str(status)

                history = get_durable_execution_history(lambda_client, durable_arn)
                new_errors = errors_from_history(history)
                if new_errors:
                    all_errors = new_errors

                # Handle callbacks
                callback_ids = extract_callback_ids(history)
                if spec.callback_result is not None or spec.callback_result_sequence is not None:
                    for idx, cid in enumerate(callback_ids):
                        if cid in callback_sent:
                            continue
                        callback_first_seen.setdefault(cid, time.time())

                        # Wait before sending to avoid racing the in-flight invocation
                        if time.time() - callback_first_seen[cid] < 8.0:
                            continue
                        try:
                            payload_bytes = callback_result_bytes_for(spec, idx)
                        except Exception:
                            continue
                        pbar.set_description("      Callback")
                        send_callback_success(lambda_client, cid, payload_bytes)
                        callback_sent.add(cid)
                        pbar.set_description("      Waiting")

                # Save artifacts
                (out_dir / f"{spec.output_key}.durable_execution.json").write_text(
                    json.dumps(exec_info, indent=2, sort_keys=True, default=str) + "\n"
                )
                (out_dir / f"{spec.output_key}.history.json").write_text(
                    json.dumps(history, indent=2, sort_keys=True, default=str) + "\n"
                )

                if status in {"SUCCEEDED", "FAILED", "TIMED_OUT", "STOPPED"}:
                    terminal = True
                    break

                time.sleep(poll_seconds)
                iteration += 1
                pbar.update(1)

        # Generate Mermaid diagram if requested
        if generate_mermaid and history:
            click.echo(styled("      Generating Mermaid diagram...", dim=True))
            mermaid_content = generate_mermaid_diagram(history)
            mermaid_file = diagrams_dir / f"{spec.bin_name}.mermaid"
            mermaid_file.write_text(mermaid_content + "\n")

            title, summary = load_example_docstring(spec.bin_name)
            source_path = f"../src/bin/{spec.bin_name}/main.rs"
            md_lines = [f"# {title}", ""]
            if summary:
                md_lines.append(summary)
                md_lines.append("")
            md_lines.append(f"Source: `{source_path}`")
            md_lines.append("")
            md_lines.append("```mermaid")
            md_lines.append(mermaid_content)
            md_lines.append("```")
            md_file = diagrams_dir / f"{spec.bin_name}.md"
            md_file.write_text("\n".join(md_lines) + "\n")

        transient_errors: list[str] = []
        terminal_error: str | None = None
        if all_errors:
            if status == "SUCCEEDED":
                transient_errors = all_errors
            else:
                terminal_error = all_errors[-1]

        results.append(
            {
                "output_key": spec.output_key,
                "function_arn": function_arn,
                "durable_execution_arn": durable_arn,
                "status": status,
                "terminal": terminal,
                "error": terminal_error,
                "transient_errors": transient_errors,
                "callbacks_sent": sorted(callback_sent),
            }
        )

        # Print final status for this example
        detail = ""
        if callback_sent:
            detail = f"{len(callback_sent)} callback(s)"
        if transient_errors:
            detail = f"{len(transient_errors)} transient error(s)" if not detail else f"{detail}, {len(transient_errors)} transient error(s)"
        print_status(example_name, status, detail)

    # Save summary
    (out_dir / "summary.json").write_text(
        json.dumps(results, indent=2, sort_keys=True) + "\n"
    )

    # Print summary table
    print_summary_table(results)

    # Print any errors in detail
    failed = [r for r in results if r["status"] != "SUCCEEDED"]
    if failed:
        print_section("Failed Examples")
        for r in failed:
            example_name = r["output_key"].replace("ExampleFunctionArn", "")
            click.echo(styled(f"  {example_name}", fg="red", bold=True))
            if r["error"]:
                click.echo(styled(f"    Error: {r['error'][:100]}...", dim=True))

    click.echo()
    return 1 if failed else 0


# -----------------------------------------------------------------------------
# CLI
# -----------------------------------------------------------------------------


@click.command()
@click.option("--region", default="us-east-1", help="AWS region")
@click.option("--stack", default="durable-rust", help="CloudFormation stack name")
@click.option(
    "--out",
    default="examples/.durable-validation",
    help="Output directory for artifacts",
)
@click.option(
    "--diagrams-out",
    default=None,
    help="Optional output directory for Mermaid/SVG diagrams (defaults to --out)",
)
@click.option(
    "--timeout-seconds",
    default=180,
    type=int,
    help="Maximum wait time per example in seconds",
)
@click.option(
    "--poll-seconds",
    default=2.0,
    type=float,
    help="Polling interval in seconds",
)
@click.option(
    "--mermaid/--no-mermaid",
    default=False,
    help="Generate Mermaid flowchart diagrams",
)
@click.option(
    "--example",
    "examples",
    multiple=True,
    help="Run only specific example(s) by output key (can be repeated)",
)
def main(
    region: str,
    stack: str,
    out: str,
    diagrams_out: str | None,
    timeout_seconds: int,
    poll_seconds: float,
    mermaid: bool,
    examples: tuple[str, ...],
) -> None:
    """Validate durable execution examples deployed via SAM."""
    diagrams_dir = Path(diagrams_out) if diagrams_out else Path(out)

    exit_code = run_validation(
        region=region,
        stack=stack,
        out_dir=Path(out),
        timeout_seconds=timeout_seconds,
        poll_seconds=poll_seconds,
        generate_mermaid=mermaid,
        diagrams_dir=diagrams_dir,
        example_filter=examples,
    )
    raise SystemExit(exit_code)


if __name__ == "__main__":
    main()
