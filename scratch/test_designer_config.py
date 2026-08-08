from tools import _load_subagents, _SUBAGENTS_REGISTRY
from llm import ChatOpenAI

def test_designer_loading():
    _load_subagents()
    assert "Designer" in _SUBAGENTS_REGISTRY
    designer = _SUBAGENTS_REGISTRY["Designer"]
    
    print("Designer registry:")
    print("  Model:", designer["model"])
    print("  Tools:", designer["tools"])
    
    assert designer["model"] == "deepseek:v4-flash"
    assert "edit_file" in designer["tools"]
    assert "view_signatures" in designer["tools"]
    assert "run_command" in designer["tools"]
    
    print("\nVerifying model thinking behavior...")
    # Test llm thinking generation logic
    # Since llm.py returns ChatOpenAI instances, we can mock/call the config generation
    # Let's inspect the returned ChatOpenAI extra_body or model arguments if possible.
    # We can invoke it with mock inputs
    from unittest.mock import patch
    with patch("llm.ChatOpenAI") as mock_chat:
        from llm import invoke_messages_with_fallback
        from langchain_core.messages import HumanMessage
        
        try:
            invoke_messages_with_fallback(
                role="Designer",
                messages=[HumanMessage(content="Hello")],
                model="deepseek:v4-pro"
            )
        except Exception as e:
            print("Caught expected/unexpected exception:", e)
            
        assert mock_chat.called
        kwargs = mock_chat.call_args[1]
        print("ChatOpenAI kwargs for role=Designer, model=deepseek:v4-pro:")
        print("  model:", kwargs.get("model"))
        print("  extra_body:", kwargs.get("extra_body"))
        print("  reasoning_effort:", kwargs.get("reasoning_effort"))
        
        assert kwargs.get("reasoning_effort") == "max"
        assert kwargs.get("extra_body") == {"thinking": {"type": "enabled"}}

if __name__ == "__main__":
    test_designer_loading()
    print("\nAll configuration assertions passed successfully!")
