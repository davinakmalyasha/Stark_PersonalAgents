import sqlite3
import unittest
from langgraph.checkpoint.base import Checkpoint
from compressed_checkpointer import CompressedSqliteSaver

class TestCheckpointerGC(unittest.TestCase):
    
    def test_garbage_collection(self) -> None:
        # 1. Establish an in-memory autocommit connection to verify Pillar 20 and Pillar 19
        conn = sqlite3.connect(":memory:", check_same_thread=False, isolation_level=None)
        saver = CompressedSqliteSaver(conn)
        saver.setup()
        
        config = {"configurable": {"thread_id": "thread-1", "checkpoint_ns": ""}}
        
        # Insert a chain of checkpoints: c1 -> c2 -> c3 -> c4
        # c4 will be the latest checkpoint.
        # c3 is parent of c4, c2 is parent of c3, c1 is parent of c2.
        # We also insert a dead branch: c2 -> c2_fork_1 -> c2_fork_2
        # where c2_fork_2 has no children and is not latest (since c4 is latest in namespace "").
        
        checkpoint1 = Checkpoint(
            v=1, id="c1", ts="2026-06-13T08:00:00Z",
            channel_values={"key": "val1"}, channel_versions={"key": 1},
            versions_seen={}, pending_sends=[]
        )
        saver.put(config, checkpoint1, {"step": 1}, {})
        
        # Now c2 (child of c1)
        config_c2 = {"configurable": {"thread_id": "thread-1", "checkpoint_ns": "", "checkpoint_id": "c1"}}
        checkpoint2 = Checkpoint(
            v=1, id="c2", ts="2026-06-13T08:01:00Z",
            channel_values={"key": "val2"}, channel_versions={"key": 2},
            versions_seen={}, pending_sends=[]
        )
        saver.put(config_c2, checkpoint2, {"step": 2}, {})
        
        # Fork branch from c2: c2_fork_1
        config_fork1 = {"configurable": {"thread_id": "thread-1", "checkpoint_ns": "", "checkpoint_id": "c2"}}
        checkpoint_fork1 = Checkpoint(
            v=1, id="c2_fork_1", ts="2026-06-13T08:02:00Z",
            channel_values={"key": "fork1"}, channel_versions={"key": 3},
            versions_seen={}, pending_sends=[]
        )
        saver.put(config_fork1, checkpoint_fork1, {"step": 3}, {})
        
        # Child of fork1: c2_fork_2
        config_fork2 = {"configurable": {"thread_id": "thread-1", "checkpoint_ns": "", "checkpoint_id": "c2_fork_1"}}
        checkpoint_fork2 = Checkpoint(
            v=1, id="c2_fork_2", ts="2026-06-13T08:03:00Z",
            channel_values={"key": "fork2"}, channel_versions={"key": 4},
            versions_seen={}, pending_sends=[]
        )
        saver.put(config_fork2, checkpoint_fork2, {"step": 4}, {})
        
        # Add a write for c2_fork_2 to verify it gets cleaned up
        saver.put_writes(
            {"configurable": {"thread_id": "thread-1", "checkpoint_ns": "", "checkpoint_id": "c2_fork_2"}},
            [("channel_w", "val_w")],
            "task_w"
        )
        
        # Verify c2_fork_2 and its write exist in DB
        cur = conn.cursor()
        cur.execute("SELECT COUNT(*) FROM checkpoints WHERE checkpoint_id = 'c2_fork_2'")
        self.assertEqual(cur.fetchone()[0], 1)
        cur.execute("SELECT COUNT(*) FROM writes WHERE checkpoint_id = 'c2_fork_2'")
        self.assertEqual(cur.fetchone()[0], 1)
        
        # Now add the active branch child of c2: c3
        config_c3 = {"configurable": {"thread_id": "thread-1", "checkpoint_ns": "", "checkpoint_id": "c2"}}
        checkpoint3 = Checkpoint(
            v=1, id="c3", ts="2026-06-13T08:04:00Z",
            channel_values={"key": "val3"}, channel_versions={"key": 5},
            versions_seen={}, pending_sends=[]
        )
        saver.put(config_c3, checkpoint3, {"step": 5}, {})
        
        # Now latest active checkpoint: c4 (child of c3)
        config_c4 = {"configurable": {"thread_id": "thread-1", "checkpoint_ns": "", "checkpoint_id": "c3"}}
        checkpoint4 = Checkpoint(
            v=1, id="c4", ts="2026-06-13T08:05:00Z",
            channel_values={"key": "val4"}, channel_versions={"key": 6},
            versions_seen={}, pending_sends=[]
        )
        
        # Put c4. Since put automatically calls gc_checkpoints,
        # it should prune the dead branch (c2_fork_1 and c2_fork_2) because:
        # - c4 is now the latest.
        # - c2_fork_2 is not the latest, and is not a parent of any checkpoint (leaf).
        # - Once c2_fork_2 is pruned, c2_fork_1 is also not the latest and no longer a parent, so it's also pruned.
        # - c1, c2, c3 are parents of the active path (c4 is latest, c3 parent of c4, c2 parent of c3, c1 parent of c2), so they are preserved.
        saver.put(config_c4, checkpoint4, {"step": 6}, {})
        
        # Verify dead branch checkpoints are pruned
        cur.execute("SELECT COUNT(*) FROM checkpoints WHERE checkpoint_id = 'c2_fork_2'")
        self.assertEqual(cur.fetchone()[0], 0)
        cur.execute("SELECT COUNT(*) FROM checkpoints WHERE checkpoint_id = 'c2_fork_1'")
        self.assertEqual(cur.fetchone()[0], 0)
        
        # Verify dead branch writes are pruned
        cur.execute("SELECT COUNT(*) FROM writes WHERE checkpoint_id = 'c2_fork_2'")
        self.assertEqual(cur.fetchone()[0], 0)
        
        # Verify active branch checkpoints are preserved
        for cid in ["c1", "c2", "c3", "c4"]:
            cur.execute("SELECT COUNT(*) FROM checkpoints WHERE checkpoint_id = ?", (cid,))
            self.assertEqual(cur.fetchone()[0], 1, f"Checkpoint {cid} should be preserved")
            
        conn.close()

if __name__ == "__main__":
    unittest.main()
