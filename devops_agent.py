"""
DevOps Subagent System Node with verification loop.
"""
import os
import json
import re
from langchain_core.messages import SystemMessage, HumanMessage, AIMessage
from developer_agent import _parse_tool_call
from devops_prompt import _STATIC_SYSTEM_TEMPLATE
from devops_verifier import verify_devops_files
from llm import invoke_messages_with_fallback
from tools import execute_tool, read_file, auto_offload_result

_DEVOPS_SYSTEM_TEMPLATE = _STATIC_SYSTEM_TEMPLATE

def devops_node(s: dict) -> dict:
    """
    DevOps execution node. Runs LLM tool execution loop,
    and validates written files (YAML, JSON, Dockerfile) before finishing.
    """
    client_req = s.get("client_request", "")
    chat_id = s.get("chat_id", "default_chat")
    
    messages = [
        SystemMessage(content=_STATIC_SYSTEM_TEMPLATE),
        HumanMessage(content=client_req)
    ]
    
    max_sub_iters = 15
    sub_iter = 0
    error_count = 0
    files_written = {}
    agent_report = ""
    devops_config = ""
    
    while sub_iter < max_sub_iters:
        sub_iter += 1
        try:
            res = invoke_messages_with_fallback(
                role="DevSecOps",
                messages=list(messages),
                temp=0.2,
                model="deepseek:v4-pro",
                reasoning_effort="max"
            )
        except Exception as e:
            agent_report = f"Error during DevOps agent LLM call: {e}"
            break
            
        add_kwargs = {}
        reasoning = getattr(res, "reasoning_content", None)
        if reasoning:
            add_kwargs["reasoning_content"] = reasoning
            
        content = str(res.content) if hasattr(res, "content") else str(res)
        messages.append(AIMessage(content=content, additional_kwargs=add_kwargs))
        
        # Parse tool calls from response
        tool_calls = []
        for match in re.finditer(r'```tool\s*\n(.*?)\n```', content, re.DOTALL):
            tc = _parse_tool_call(match.group(0))
            if tc:
                tool_calls.append(tc)
        if not tool_calls:
            tc = _parse_tool_call(content)
            if tc:
                tool_calls.append(tc)
                
        if not tool_calls:
            # End of turn verification
            validation_errors = verify_devops_files(files_written)
            if validation_errors:
                error_count += 1
                if error_count < 3:
                    err_msg = "[SYSTEM VERIFICATION ERROR] DevOps changes failed safety or syntax validation:\n"
                    err_msg += "\n".join(f"- {e}" for e in validation_errors)
                    err_msg += "\n\nPlease correct these issues immediately by rewriting or editing the files."
                    messages.append(HumanMessage(content=err_msg))
                    continue
                else:
                    agent_report = f"DevOps agent execution failed due to persistent verification issues:\n" + "\n".join(validation_errors)
                    break
            else:
                agent_report = content
                if files_written:
                    devops_config = "\n\n".join(f"### File: {p}\n{c}" for p, c in files_written.items())
                break
                
        # Execute tool calls
        tool_results = []
        for tc in tool_calls:
            tool_name = tc.get("tool", "")
            args = tc.get("args", {})
            
            allowed_tools = ["run_command", "write_file", "read_file", "list_files",
                             "edit_file", "search_code",
                             "github_create_repo", "github_create_issue",
                             "github_create_pull_request", "github_list_issues",
                             "github_create_or_update_file", "github_get_file_contents",
                             "github_create_branch", "github_merge_pull_request",
                             "github_get_pull_request_files", "github_add_issue_comment"]
            if tool_name not in allowed_tools:
                tool_results.append(f"[SKIPPED] Tool '{tool_name}' not in subagent permissions.")
                continue
                
            try:
                # Intercept write_file args for validation
                if tool_name == "write_file" and "file_path" in args and "content" in args:
                    fpath = args["file_path"].replace("\\", "/")
                    files_written[fpath] = args["content"]
                    
                result = execute_tool(tool_name, args)
                
                # Intercept edit_file to fetch final contents
                if tool_name in ("edit_file", "apply_diff") and "file_path" in args:
                    fpath = args["file_path"].replace("\\", "/")
                    try:
                        files_written[fpath] = read_file(fpath)
                    except Exception:
                        pass
                        
                result = auto_offload_result(str(result), tool_name, max_chars=12000)
                tool_results.append(f"[{tool_name}] {result}")
            except Exception as e:
                tool_results.append(f"[{tool_name}] ERROR: {str(e)[:200]}")
                
        feedback = "\n".join(tool_results) if tool_results else "No tool results."
        messages.append(HumanMessage(content=f"Tool results:\n{feedback}"))
        
    return {
        "agent_report": agent_report,
        "devops_config": devops_config,
        "messages": messages
    }
