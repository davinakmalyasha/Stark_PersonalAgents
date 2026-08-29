"""
Ask / confirm router â€” Phase 7 moved from server/api_server.py:1402-1421.
"""
from fastapi import APIRouter, Request; from fastapi.responses import JSONResponse
from pydantic import BaseModel

router = APIRouter(tags=["ask"])


class AskAnswerRequest(BaseModel):
    batch_id: str = ""
    answers: list[dict] = []


@router.get("/api/ask/pending")
async def api_ask_pending(chat_id: str = ""):
    from user_questions import pending_batches
    return JSONResponse(content={"batches": pending_batches()})


@router.post("/api/ask/answer")
async def api_ask_answer(req: AskAnswerRequest):
    from user_questions import resolve_batch
    try:
        resolved = resolve_batch(req.batch_id, req.answers)
        return JSONResponse(content={"ok": True, "resolved": resolved})
    except Exception as e:
        return JSONResponse(content={"ok": False, "error": str(e)}, status_code=400)



