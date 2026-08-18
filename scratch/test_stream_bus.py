"""P10 streaming smoke test - no real LLM calls.

Fakes langchain's client to emit streamed chunks (text + reasoning) and verifies:
1. invoke_messages_with_fallback_stream pushes deltas into the StreamBus
2. the returned value matches the non-streaming shape
3. TOKEN_STREAMING=0 falls back to the non-streaming path

Run: venv312\\Scripts\\python.exe scratch\\test_stream_bus.py
"""
import sys

sys.path.insert(0, ".")

FAIL = []


def check(name, cond, detail=""):
    status = "PASS" if cond else "FAIL"
    print(f"  [{status}] {name}{' - ' + detail if detail and not cond else ''}")
    if not cond:
        FAIL.append(name)


def main():
    from sync import state_sync
    state_sync.shared_state["chat_id"] = "chat-test"
    state_sync.shared_state["selected_temp"] = 0.7

    import stream_bus
    stream_bus.clear("chat-test")

    import llm

    class FakeChunk:
        def __init__(self, content="", reasoning=None):
            self.content = content
            self.additional_kwargs = {}
            if reasoning:
                self.additional_kwargs["reasoning_content"] = reasoning

    class FakeLLM:
        stream_usage = True

        def stream(self, messages, config=None):
            yield FakeChunk("Hello ", reasoning="thinking step 1")
            yield FakeChunk("world", reasoning=" thinking step 2")
            yield FakeChunk("!")

        def invoke(self, messages, config=None):
            return FakeChunk("Hello world!")

    orig_client = llm._get_deepseek_client
    llm._get_deepseek_client = lambda *a, **k: FakeLLM()
    orig_enabled = stream_bus.enabled
    try:
        # 1: streaming emits deltas + correct return shape
        stream_bus.enabled = lambda: True
        res = llm.invoke_messages_with_fallback_stream(role="Dev", messages=[], where="test", temp=0.2)
        check("stream returns StrWithMetadata text", isinstance(res, str) and res == "Hello world!")
        check("reasoning attached", getattr(res, "reasoning_content", None) == "thinking step 1 thinking step 2")
        deltas = stream_bus.drain("chat-test")
        kinds = [d["kind"] for d in deltas]
        texts = "".join(d["text"] for d in deltas if d["kind"] == "text")
        reasons = "".join(d["text"] for d in deltas if d["kind"] == "reasoning")
        check("text deltas emitted", "text" in kinds and texts == "Hello world!")
        check("reasoning deltas emitted", "reasoning" in kinds and reasons == "thinking step 1 thinking step 2")
        check("end marker emitted", "end" in kinds)
        check("bus drained (empty after)", stream_bus.drain("chat-test") == [])

        # 2: tools mode returns (text, tool_calls, reasoning) tuple
        res2 = llm.invoke_messages_with_fallback_stream(
            role="Dev", messages=[], tools=[{"type": "function", "function": {"name": "x"}}], where="t", temp=0.2,
        )
        check("tools mode returns tuple", isinstance(res2, tuple) and len(res2) == 3)
        stream_bus.clear("chat-test")

        # 3: TOKEN_STREAMING=0 -> non-streaming fallback, no bus deltas
        stream_bus.enabled = lambda: False
        res3 = llm.invoke_messages_with_fallback_stream(role="Dev", messages=[], where="t2", temp=0.2)
        check("disabled -> non-streaming text", isinstance(res3, str) and res3 == "Hello world!")
        check("no deltas when disabled", stream_bus.drain("chat-test") == [])

        # 4: stream raising mid-way -> graceful non-streaming fallback
        class BoomLLM(FakeLLM):
            def stream(self, messages, config=None):
                yield FakeChunk("partial")
                raise RuntimeError("boom")

        llm._get_deepseek_client = lambda *a, **k: BoomLLM()
        stream_bus.enabled = lambda: True
        try:
            res4 = llm.invoke_messages_with_fallback_stream(role="Dev", messages=[], where="t3", temp=0.2)
            check("stream failure falls back gracefully", isinstance(res4, str))
        except Exception as e:
            check("stream failure falls back gracefully", False, str(e))
        stream_bus.clear("chat-test")
    finally:
        llm._get_deepseek_client = orig_client
        stream_bus.enabled = orig_enabled


if __name__ == "__main__":
    main()
    print()
    if FAIL:
        print(f"FAILED: {len(FAIL)} - {', '.join(FAIL)}")
        sys.exit(1)
    print("ALL PASS")
