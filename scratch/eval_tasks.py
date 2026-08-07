# 5th Generation Evaluation Task Definitions (Adaptive Consulting Intelligence)
# Rewritten to represent real-world daily coding requests and developer tasks.
# Simplified prompts to be human-like, high-level requests, with NO filenames mentioned in prompts.

UNIT_TASKS = [
    {
        "id": 1,
        "name": "Thread-Safe Concurrent Cache (LRU & TTL)",
        "prompt": "Build a thread-safe concurrent in-memory cache in Python. It needs to support LRU eviction, TTL expiration for keys, and backing up / restoring to a JSON file. Write a set of tests to verify it works correctly under concurrent thread pressure.",
        "verify_cmd": "python -m pytest test_concurrent_cache.py",
        "expected_files": ["concurrent_cache.py", "test_concurrent_cache.py"]
    },
    {
        "id": 2,
        "name": "Rich Markdown Compiler to Semantic HTML",
        "prompt": "Write a markdown to HTML compiler in Python. It should support headings, bold/italic inline styling, nested bullet and numbered lists, and full markdown tables. Create unit tests to check all of these formatting rules.",
        "verify_cmd": "python -m pytest test_markdown_compiler.py",
        "expected_files": ["markdown_compiler.py", "test_markdown_compiler.py"]
    },
    {
        "id": 3,
        "name": "Custom Async Event Loop & Task Runner",
        "prompt": "I want a custom asynchronous event loop and task runner in Python without using the built-in asyncio library. It needs to run coroutines/generators, handle non-blocking sleeps, and support offloading blocking tasks to a thread pool. Write tests to verify it works.",
        "verify_cmd": "python -m pytest test_custom_event_loop.py",
        "expected_files": ["custom_event_loop.py", "test_custom_event_loop.py"]
    },
    {
        "id": 4,
        "name": "Sliding Window Log Rate Limiter",
        "prompt": "Create a sliding window log rate limiter in Python that limits client requests (supporting multiple client keys) and persists request logs to a SQLite database. Write tests to verify the limits.",
        "verify_cmd": "python -m pytest test_rate_limiter.py",
        "expected_files": ["rate_limiter.py", "test_rate_limiter.py"]
    },
    {
        "id": 5,
        "name": "In-Memory Relational Engine",
        "prompt": "Implement an in-memory SQL-like relational engine in Python. It should parse custom query structures, execute INNER and LEFT joins, and optimize the execution plan (e.g. by filtering before joining). Write tests to verify its correctness.",
        "verify_cmd": "python -m pytest test_query_engine.py",
        "expected_files": ["query_engine.py", "test_query_engine.py"]
    },
    {
        "id": 6,
        "name": "Time-Expanded A* Pathfinder",
        "prompt": "Implement a time-expanded A* pathfinding solver in Python that finds the shortest path on a 2D grid while avoiding static and dynamic (moving) obstacles over time. Add tests to verify dynamic obstacle avoidance.",
        "verify_cmd": "python -m pytest test_astar_3d.py",
        "expected_files": ["astar_3d.py", "test_astar_3d.py"]
    },
    {
        "id": 7,
        "name": "Git Simulator Engine",
        "prompt": "Build a simple local version control simulator in Python. It should support git-like commands: initialize a repo, add/stage files, commit changes, show status, and checkout past commits. Write tests to verify commits and rollbacks.",
        "verify_cmd": "python -m pytest test_vcs_engine.py",
        "expected_files": ["vcs_engine.py", "test_vcs_engine.py"]
    },
    {
        "id": 8,
        "name": "DAG Execution Engine",
        "prompt": "Write a task execution engine in Python that runs tasks with dependencies concurrently using a thread pool. It must check for cycles in the dependency graph and handle task failures. Test it with unit tests.",
        "verify_cmd": "python -m pytest test_dag_engine.py",
        "expected_files": ["dag_engine.py", "test_dag_engine.py"]
    },
    {
        "id": 16,
        "name": "RESTful Todo API (Node.js/Express)",
        "prompt": "Build a RESTful TODO list API in Node.js using Express. The API should persist tasks in a local SQLite database (todo.db). It must support endpoints for: creating a task (POST /tasks), retrieving all tasks (GET /tasks), updating a task's title and status (PUT /tasks/{id}), and deleting a task (DELETE /tasks/{id}). Validate input: title is required and must be 1-200 chars. Write a Node.js test script `test_todo_api.js` using the native 'assert' module (or a library like Jest) that spins up the server, calls all endpoints using fetch/http, and verifies correctness.",
        "verify_cmd": "node test_todo_api.js",
        "expected_files": ["todo_api.js", "test_todo_api.js", "package.json"]
    },
    {
        "id": 17,
        "name": "Dynamic Interactive Dashboard (HTML/CSS/JS)",
        "prompt": "Create a pure frontend interactive dashboard page (`index.html`, `dashboard.js`, `styles.css`) for project management. It should support: displaying a list of tasks, adding/deleting tasks dynamically in the DOM, sorting tasks by priority (High/Medium/Low), and persisting the tasks to LocalStorage. It must look professional, clean, and modern. Write a Node.js test script `test_dashboard.js` that checks for the existence of the HTML, JS, and CSS files, reads and parses the HTML using regex/dom parsing to verify the necessary elements (like task list, sort button) exist, and validates the JS functions.",
        "verify_cmd": "node test_dashboard.js",
        "expected_files": ["index.html", "dashboard.js", "styles.css", "test_dashboard.js"]
    },
    {
        "id": 18,
        "name": "TypeScript Utility: JWT Middleware",
        "prompt": "Write a Node.js/TypeScript Express middleware for JWT authentication and role-based access control. It should export a function `requireAuth(allowedRoles: string[])` that validates the 'Authorization' header for a bearer JWT token (verifying signature and expiration), decodes the payload (containing username and role), and attaches it to the request. If the token is missing, expired, or doesn't have the required role, return appropriate 401/403 errors. Write a test file `test_middleware.ts` that runs tests verifying all these middleware rules. Since TypeScript is used, provide a working `tsconfig.json` and package configurations.",
        "verify_cmd": "npx ts-node test_middleware.ts",
        "expected_files": ["middleware.ts", "test_middleware.ts", "tsconfig.json", "package.json"]
    },
    {
        "id": 19,
        "name": "E-Commerce Shopping Cart API (PHP)",
        "prompt": "Build an object-oriented shopping cart class and API in PHP. The `Cart` class should support adding items (with quantity), removing items, updating quantity, calculating the subtotal, applying a percentage discount coupon, and calculating a 10% tax on the discounted subtotal. Write a PHP script `CartTest.php` that uses native PHP `assert()` statements to thoroughly verify the Cart class correctness (adding, removing, quantity updates, discounts, taxes).",
        "verify_cmd": "php CartTest.php",
        "expected_files": ["Cart.php", "CartTest.php"]
    },
    {
        "id": 20,
        "name": "SQL Schema Migrator (Node.js)",
        "prompt": "Create a database schema migrator utility in Node.js/JavaScript. It should read schema migration files from a folder (`migrations/001_init.sql`, `migrations/002_add_status.sql` etc.), apply them in sequence to an SQLite database, and track the current version of the schema in a `schema_version` table in the database so it never runs the same migration twice. It must support applying new migrations (up) and rollback of migrations (down). Write a test script `test_migrator.js` that tests applying and rolling back migrations and confirms the SQLite database schema matches the expected state.",
        "verify_cmd": "node test_migrator.js",
        "expected_files": ["migrator.js", "test_migrator.js"]
    }
]

INTERACTIVE_TASKS = [
    {
        "id": 9,
        "name": "Resilient Client",
        "type": "self_heal",
        "prompt": "Create a resilient HTTP client wrapper in Python that handles timeouts, connection errors, and retries failed requests with exponential backoff. It should also implement a circuit breaker pattern. Write tests to verify this.",
        "verify_cmd": "python -m pytest test_resilient_client.py",
        "expected_files": ["resilient_client.py", "test_resilient_client.py"]
    },
    {
        "id": 10,
        "name": "Secure Password Vault",
        "type": "pivot",
        "prompt": "Design a secure password storage manager in Python that encrypts and decrypts credentials. Show me the design choices first.",
        "pivot_prompt": "Now, add a key-derivation iteration-backoff mechanism to the PasswordVault to prevent brute-force attacks. Each failed attempt should double the backoff/work. Update the code and tests to verify this.",
        "verify_cmd": "python -m pytest test_vault.py",
        "expected_files": ["vault.py", "test_vault.py"]
    },
    {
        "id": 11,
        "name": "HTTP Router with Middlewares",
        "type": "carryover",
        "prompt": "I need a custom HTTP request router in Python that matches methods and paths, supporting params like `/users/{id}`. Propose design choices.",
        "followup_prompt": "Now add global and route-specific middleware chain support. Middlewares should take request and next_fn. Update the router and write tests.",
        "verify_cmd": "python -m pytest test_router.py",
        "expected_files": ["router.py", "test_router.py"]
    },
    {
        "id": 12,
        "name": "Dynamic Port Scanning Server",
        "type": "port_conflict",
        "prompt": "Start a simple HTTP server in Python with a `/health` endpoint. If port 8000 is occupied, it should scan for a free port, start there, and save the port to a JSON file.",
        "verify_cmd": "",
        "expected_files": ["web_server.py"]
    },
    {
        "id": 13,
        "name": "Database Schema Compiler",
        "type": "vague_probe",
        "prompt": "I need a compiler that parses custom schema DSL strings and outputs SQL DDL statements. Propose some design choices.",
        "verify_cmd": "",
        "expected_files": ["schema_plan.md"]
    },
    {
        "id": 14,
        "name": "Billing Refactoring",
        "type": "refactor",
        "prompt": "Refactor the messy billing code. Show me the architectural options.",
        "verify_cmd": "python -m pytest test_billing.py",
        "expected_files": ["billing.py", "test_billing.py", "domain/order.py", "domain/tax.py", "domain/discount.py", "domain/invoice.py"]
    },
    {
        "id": 15,
        "name": "A* Pathfinding Solver",
        "type": "resume",
        "prompt": "Implement a 2D A* pathfinder in Python that finds the shortest path on a grid of obstacles. Write tests.",
        "verify_cmd": "python -m pytest test_astar.py",
        "expected_files": ["astar.py", "test_astar.py"]
    }
]
