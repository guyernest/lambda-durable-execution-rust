# /// script
# requires-python = ">=3.11"
# dependencies = [
#     "click>=8.0",
#     "boto3>=1.35",
# ]
# ///
"""
MCP Agent end-to-end validator using boto3 and Click.

Seeds AgentRegistry, invokes the deployed mcp_agent Lambda,
validates the AgentResponse structure, and cleans up.

Usage:
    uv run examples/scripts/validate_agent.py --mcp-server-url https://mcp.example.com
    uv run examples/scripts/validate_agent.py --mcp-server-url https://mcp.example.com --keep-config
"""
from __future__ import annotations

import json
import sys
import time
from typing import Any

import boto3
import click


# -----------------------------------------------------------------------------
# Output Styling (same conventions as validate.py)
# -----------------------------------------------------------------------------

BOX_TL = "+"
BOX_TR = "+"
BOX_BL = "+"
BOX_BR = "+"
BOX_H = "-"
BOX_V = "|"

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
    click.echo(
        styled(BOX_V, fg="cyan")
        + " " * padding
        + styled(title, fg="cyan", bold=True)
        + " " * (width - padding - len(title) - 2)
        + styled(BOX_V, fg="cyan")
    )
    click.echo(styled(BOX_BL + BOX_H * (width - 2) + BOX_BR, fg="cyan"))


def print_section(title: str) -> None:
    """Print a section header."""
    click.echo()
    click.echo(styled(f"{SYM_ARROW} {title}", fg="yellow", bold=True))
    click.echo(styled("-" * 50, fg="yellow", dim=True))


def print_status(label: str, ok: bool, detail: str = "") -> None:
    """Print a status line with colored indicator."""
    if ok:
        symbol = styled(SYM_OK, fg="green", bold=True)
    else:
        symbol = styled(SYM_FAIL, fg="red", bold=True)
    line = f"  {symbol} {label}"
    if detail:
        line += styled(f" ({detail})", dim=True)
    click.echo(line)


# -----------------------------------------------------------------------------
# AWS API Functions
# -----------------------------------------------------------------------------


def get_stack_outputs(cfn_client: Any, stack_name: str) -> dict[str, str]:
    """Get CloudFormation stack outputs as a dictionary."""
    resp = cfn_client.describe_stacks(StackName=stack_name)
    outputs = resp["Stacks"][0].get("Outputs", [])
    return {o["OutputKey"]: o["OutputValue"] for o in outputs}


def seed_agent_config(
    dynamodb_client: Any,
    table_name: str,
    agent_name: str,
    version: str,
    mcp_server_url: str,
) -> None:
    """Seed the AgentRegistry DynamoDB table with a test agent configuration."""
    dynamodb_client.put_item(
        TableName=table_name,
        Item={
            "agent_name": {"S": agent_name},
            "version": {"S": version},
            "system_prompt": {
                "S": "You are a helpful assistant. Use the available tools to answer the user's question. Be concise."
            },
            "llm_provider": {"S": "anthropic"},
            "llm_model": {"S": "claude-sonnet-4-20250514"},
            "parameters": {
                "S": json.dumps(
                    {
                        "max_iterations": 5,
                        "temperature": 0.0,
                        "max_tokens": 1024,
                        "timeout_seconds": 120,
                    }
                )
            },
            "mcp_servers": {"S": json.dumps([mcp_server_url])},
        },
    )


def invoke_agent(
    lambda_client: Any,
    function_arn: str,
    agent_name: str,
    version: str,
    user_message: str,
) -> str:
    """Invoke the mcp_agent Lambda asynchronously and return the DurableExecutionArn."""
    payload = {
        "agent_name": agent_name,
        "version": version,
        "messages": [{"role": "user", "content": user_message}],
    }
    resp = lambda_client.invoke(
        FunctionName=function_arn,
        Qualifier="$LATEST",
        InvocationType="Event",
        Payload=json.dumps(payload).encode("utf-8"),
    )
    durable_arn = resp.get("DurableExecutionArn")
    if not isinstance(durable_arn, str) or not durable_arn:
        raise RuntimeError(
            f"Invoke did not return DurableExecutionArn: {resp}"
        )
    return durable_arn


def poll_execution(
    lambda_client: Any,
    durable_execution_arn: str,
    timeout_seconds: int,
    poll_interval: float,
) -> dict[str, Any]:
    """Poll a durable execution until it reaches a terminal state."""
    start_time = time.time()
    last_status = "UNKNOWN"

    while time.time() - start_time < timeout_seconds:
        try:
            exec_info = lambda_client.get_durable_execution(
                DurableExecutionArn=durable_execution_arn,
            )
        except Exception:
            time.sleep(poll_interval)
            continue

        last_status = exec_info.get("Status", "UNKNOWN")
        click.echo(
            styled(f"      Status: {last_status}", dim=True),
            nl=True,
        )

        if last_status in {"SUCCEEDED", "FAILED", "TIMED_OUT", "STOPPED"}:
            return exec_info

        time.sleep(poll_interval)

    return {"Status": last_status, "timeout": True}


def get_execution_result(
    lambda_client: Any,
    durable_execution_arn: str,
) -> Any | None:
    """Extract the final result from a succeeded durable execution's history."""
    try:
        history = lambda_client.get_durable_execution_history(
            DurableExecutionArn=durable_execution_arn,
            IncludeExecutionData=True,
        )
    except Exception as e:
        click.echo(styled(f"      Failed to get history: {e}", fg="red"))
        return None

    # Walk events looking for ExecutionSucceeded with output data
    for event in history.get("Events", []):
        event_type = event.get("EventType", "")
        if event_type == "ExecutionSucceeded":
            details = event.get("ExecutionSucceededDetails", {})
            output_data = details.get("Output")
            if output_data:
                try:
                    return json.loads(output_data)
                except (json.JSONDecodeError, TypeError):
                    return output_data
    return None


def validate_response(result: Any) -> list[str]:
    """Validate the AgentResponse JSON structure. Returns list of errors (empty = pass)."""
    errors: list[str] = []

    if not isinstance(result, dict):
        errors.append(f"Expected dict, got {type(result).__name__}")
        return errors

    # Check top-level 'message' key (from flattened LLMResponse)
    if "message" not in result:
        errors.append("Missing top-level 'message' key")

    # Check top-level 'metadata' key (from flattened LLMResponse)
    if "metadata" not in result:
        errors.append("Missing top-level 'metadata' key")
    else:
        metadata = result["metadata"]
        if not isinstance(metadata, dict):
            errors.append(f"'metadata' should be dict, got {type(metadata).__name__}")
        else:
            model_id = metadata.get("model_id")
            if not isinstance(model_id, str) or not model_id:
                errors.append("'metadata.model_id' should be a non-empty string")
            if "stop_reason" not in metadata:
                errors.append("'metadata.stop_reason' is missing")

    # Check 'agent_metadata' key (observability data)
    if "agent_metadata" not in result:
        errors.append("Missing 'agent_metadata' key")
    else:
        am = result["agent_metadata"]
        if not isinstance(am, dict):
            errors.append(f"'agent_metadata' should be dict, got {type(am).__name__}")
        else:
            if am.get("iterations", 0) < 1:
                errors.append("'agent_metadata.iterations' should be >= 1")
            if am.get("total_input_tokens", 0) <= 0:
                errors.append("'agent_metadata.total_input_tokens' should be > 0")
            if am.get("total_output_tokens", 0) <= 0:
                errors.append("'agent_metadata.total_output_tokens' should be > 0")
            if am.get("elapsed_ms", 0) <= 0:
                errors.append("'agent_metadata.elapsed_ms' should be > 0")

    return errors


def cleanup_agent_config(
    dynamodb_client: Any,
    table_name: str,
    agent_name: str,
    version: str,
) -> None:
    """Delete the seeded agent config item from AgentRegistry."""
    dynamodb_client.delete_item(
        TableName=table_name,
        Key={
            "agent_name": {"S": agent_name},
            "version": {"S": version},
        },
    )


# -----------------------------------------------------------------------------
# Main Validation Flow
# -----------------------------------------------------------------------------


def run_validation(
    region: str,
    profile: str | None,
    stack: str,
    mcp_server_url: str,
    agent_name: str,
    version: str,
    timeout_seconds: int,
    poll_seconds: float,
    keep_config: bool,
) -> int:
    """Run the end-to-end agent validation. Returns 0 on pass, 1 on fail."""
    print_header("MCP AGENT VALIDATOR")

    # Initialize AWS clients
    click.echo()
    click.echo(styled("  Initializing...", dim=True))
    session = boto3.Session(region_name=region, profile_name=profile)
    cfn_client = session.client("cloudformation")
    lambda_client = session.client("lambda")
    dynamodb_client = session.client("dynamodb")

    click.echo(styled(f"  Region: {region}", dim=True))
    click.echo(styled(f"  Stack: {stack}", dim=True))
    click.echo(styled(f"  MCP Server: {mcp_server_url}", dim=True))

    # Load stack outputs
    print_section("Loading Stack Outputs")
    try:
        outputs = get_stack_outputs(cfn_client, stack)
    except Exception as e:
        click.echo(styled(f"  Failed to load stack outputs: {e}", fg="red"))
        return 1

    function_arn = outputs.get("McpAgentFunctionArn")
    table_name = outputs.get("AgentRegistryTableName")

    if not function_arn:
        click.echo(styled("  Missing stack output: McpAgentFunctionArn", fg="red"))
        return 1
    if not table_name:
        click.echo(styled("  Missing stack output: AgentRegistryTableName", fg="red"))
        return 1

    print_status("McpAgentFunctionArn", True, function_arn)
    print_status("AgentRegistryTableName", True, table_name)

    # Seed agent config
    print_section("Seeding Agent Configuration")
    try:
        seed_agent_config(dynamodb_client, table_name, agent_name, version, mcp_server_url)
        print_status("Agent config seeded", True, f"{agent_name}/{version}")
    except Exception as e:
        click.echo(styled(f"  Failed to seed agent config: {e}", fg="red"))
        return 1

    # From here, always clean up (unless --keep-config)
    result_json: Any = None
    exec_status = "UNKNOWN"
    validation_errors: list[str] = []

    try:
        # Invoke agent
        print_section("Invoking MCP Agent")
        user_message = "What tools do you have available? List them briefly."
        click.echo(styled(f"  Message: \"{user_message}\"", dim=True))

        try:
            durable_arn = invoke_agent(
                lambda_client, function_arn, agent_name, version, user_message
            )
            print_status("Lambda invoked", True, "async")
            click.echo(styled(f"      DurableExecutionArn: {durable_arn}", dim=True))
        except Exception as e:
            click.echo(styled(f"  Failed to invoke agent: {e}", fg="red"))
            return 1

        # Poll for completion
        print_section("Polling Execution")
        exec_info = poll_execution(lambda_client, durable_arn, timeout_seconds, poll_seconds)
        exec_status = exec_info.get("Status", "UNKNOWN")

        if exec_status == "SUCCEEDED":
            print_status("Execution completed", True, exec_status)

            # Extract result
            print_section("Extracting Result")
            result_json = get_execution_result(lambda_client, durable_arn)

            if result_json is None:
                click.echo(styled("  Could not extract result from history", fg="red"))
                validation_errors.append("No result extracted from execution history")
            else:
                click.echo(styled("  Result extracted successfully", dim=True))
                click.echo(
                    styled(
                        f"  Response preview: {json.dumps(result_json, indent=2)[:500]}",
                        dim=True,
                    )
                )

                # Validate response structure
                print_section("Validating Response Structure")
                validation_errors = validate_response(result_json)

                if not validation_errors:
                    print_status("Response structure", True, "all checks passed")
                else:
                    for err in validation_errors:
                        print_status(err, False)

        elif exec_status in {"FAILED", "TIMED_OUT", "STOPPED"}:
            print_status("Execution completed", False, exec_status)
            validation_errors.append(f"Execution ended with status: {exec_status}")

            # Try to get error details
            error_info = exec_info.get("Error")
            if error_info:
                click.echo(styled(f"  Error: {json.dumps(error_info, default=str)[:200]}", fg="red"))
        else:
            print_status("Execution timed out waiting", False, f"last status: {exec_status}")
            validation_errors.append(f"Timed out waiting for completion (last status: {exec_status})")

    finally:
        # Clean up agent config
        if not keep_config:
            print_section("Cleaning Up")
            try:
                cleanup_agent_config(dynamodb_client, table_name, agent_name, version)
                print_status("Agent config deleted", True, f"{agent_name}/{version}")
            except Exception as e:
                click.echo(styled(f"  Warning: cleanup failed: {e}", fg="yellow"))
        else:
            click.echo()
            click.echo(styled("  Skipping cleanup (--keep-config)", dim=True))

    # Print summary
    click.echo()
    inner_width = 40
    click.echo(styled(BOX_TL + BOX_H * inner_width + BOX_TR, fg="cyan"))
    click.echo(
        styled(BOX_V, fg="cyan")
        + styled("  VALIDATION SUMMARY", bold=True).ljust(inner_width)
        + styled(BOX_V, fg="cyan")
    )
    click.echo(styled(BOX_V + BOX_H * inner_width + BOX_V, fg="cyan"))

    status_line = f"  Status: {exec_status}"
    click.echo(styled(BOX_V, fg="cyan") + status_line.ljust(inner_width) + styled(BOX_V, fg="cyan"))

    if validation_errors:
        err_line = f"  Errors: {len(validation_errors)}"
        click.echo(
            styled(BOX_V, fg="cyan")
            + styled(err_line, fg="red").ljust(inner_width)
            + styled(BOX_V, fg="cyan")
        )
        result_line = "  Result: FAIL"
        click.echo(
            styled(BOX_V, fg="cyan")
            + styled(result_line, fg="red", bold=True).ljust(inner_width)
            + styled(BOX_V, fg="cyan")
        )
    else:
        result_line = "  Result: PASS"
        click.echo(
            styled(BOX_V, fg="cyan")
            + styled(result_line, fg="green", bold=True).ljust(inner_width)
            + styled(BOX_V, fg="cyan")
        )

    click.echo(styled(BOX_BL + BOX_H * inner_width + BOX_BR, fg="cyan"))
    click.echo()

    if validation_errors:
        click.echo(styled("  Validation failed. See details above.", fg="red", bold=True))
        return 1
    else:
        click.echo(styled("  Agent validation passed!", fg="green", bold=True))
        return 0


# -----------------------------------------------------------------------------
# CLI
# -----------------------------------------------------------------------------


@click.command()
@click.option("--region", default="us-east-1", help="AWS region")
@click.option("--profile", default=None, help="AWS CLI profile name")
@click.option("--stack", default="durable-rust", help="CloudFormation stack name")
@click.option(
    "--mcp-server-url",
    required=True,
    help="URL of MCP server to validate against",
)
@click.option("--agent-name", default="test-agent", help="Agent name for test config")
@click.option("--version", default="v1", help="Agent version for test config")
@click.option(
    "--timeout-seconds",
    default=240,
    type=int,
    help="Maximum wait time for execution in seconds",
)
@click.option(
    "--poll-seconds",
    default=3.0,
    type=float,
    help="Polling interval in seconds",
)
@click.option(
    "--keep-config",
    is_flag=True,
    help="Don't delete AgentRegistry item after validation",
)
def main(
    region: str,
    profile: str | None,
    stack: str,
    mcp_server_url: str,
    agent_name: str,
    version: str,
    timeout_seconds: int,
    poll_seconds: float,
    keep_config: bool,
) -> None:
    """Validate a deployed MCP Agent Lambda end-to-end.

    Seeds the AgentRegistry DynamoDB table with a test agent configuration,
    invokes the mcp_agent Lambda, validates the response structure, and
    cleans up.
    """
    exit_code = run_validation(
        region=region,
        profile=profile,
        stack=stack,
        mcp_server_url=mcp_server_url,
        agent_name=agent_name,
        version=version,
        timeout_seconds=timeout_seconds,
        poll_seconds=poll_seconds,
        keep_config=keep_config,
    )
    raise SystemExit(exit_code)


if __name__ == "__main__":
    main()
