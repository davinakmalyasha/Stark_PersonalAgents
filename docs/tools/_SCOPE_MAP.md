# _SCOPE_MAP (tools/_schemas.py)

Generated from the runtime `_SCOPE_MAP`. The canonical machine-readable copy is
`backend-rs/specs/tool-specs.json`.

## developer

- read_file
- edit_file
- run_command
- search_code
- ask_user
- update_plan
- call_subagent

## general

- run_command
- read_pdf
- create_docx
- create_xlsx
- ask_user
- update_plan
- browser_act
- files_changed
- look_at_screen
- set_screen_share

## house

- house_get_state
- house_get_device
- house_set_device
- house_run_scene
- house_get_history
- run_command
- browser_act
- ask_user
- update_plan

## dfd

- dfd_bridge
- create_client
- create_coding_project
- run_command
- ask_user
- update_plan
- files_changed

## orchestrator

- create_coding_project
- ask_user
- delegate
- delegation_status
- job_message
- job_cancel
- create_group
- group_board
- group_item_detail
- group_events
- group_update
- schedule
- give_note
- canvas_mode

## devops

- github_apply_gate
- run_command
- read_file
- edit_file
- search_code
- update_plan

## explore

- read_file
- search_code
- run_command

## ui

- browser_act
- read_file
- search_code
- run_command
- edit_file
- ask_user
- update_plan

## Not registered

`propose_plan` is implemented in `tools/MainTool/proposePlan.py` and allowed by
the plan-mode guard, but it is not in `TOOL_DEFINITIONS` or `_SCOPE_MAP`; the
native model-facing schema never exposes it.
