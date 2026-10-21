import pathlib, re
p = pathlib.Path(r"D:\MyProject\LangChain\frontend\src\components\Workspace\MessageBubble.tsx")
t = p.read_text(encoding='utf-8')
# fix liveTimeline refs
t = t.replace("!!liveTimeline.length ? (\n                <LiveTimeline segments={liveTimeline} />",
             "!!timelineSegments.length ? (\n                <LiveTimeline segments={timelineSegments} />")
# remove persisted thought block
import re
t = re.sub(r"\s*\{/\* Persisted thought[^}]*\}\s*\n\s*\{!streamingActive[^}]*?/>\s*\)\}", "", t, flags=re.DOTALL)
old_p10 = """          {/* P10: live token streaming — interleaved step timeline.
              Reasoning now renders as its own segments inside the timeline
              (one blob was the \"everything in one line\" bug); the accumulated
              reasoning is only kept for persistence (thinking metadata).
              Thinking shows immediately on send, before the first delta. */}
          {message.duration === 'active' && (
            <>
              {!!timelineSegments.length ? (
                <LiveTimeline segments={timelineSegments} />
              ) : !message.content ? (
                <ThinkingPlaceholder />
              ) : null}
            </>
          )}"""
new_p10 = """          {/* Timeline — live and history share same component. Live: reasoning/tool interleaved;
              before first delta show a light Thinking placeholder. History: persisted timeline, so reasoning doesn't vanish. */}
          {timelineSegments.length ? (
            <LiveTimeline segments={timelineSegments} />
          ) : message.duration === 'active' && !message.content ? (
            <ThinkingPlaceholder />
          ) : null}"""
if old_p10 in t:
    t = t.replace(old_p10, new_p10)
    print("replaced P10 block")
else:
    print("P10 block not found, trying alt")
    # fallback manual
    if "timelineSegments.length ?" in t:
        print("already has timelineSegments")
p.write_text(t, encoding='utf-8')
print("patched")
print("liveTimeline" in t)
print("timelineSegments" in t)
print("THOUGHT FOR" in t)
