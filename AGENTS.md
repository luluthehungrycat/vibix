# AGENTS.md
*Guidelines for using coding agents (Vibe CLI, Web Vibe, OpenCode) with VIBIX.*

---

## **🎯 General Rules**
1. **Primary Tool**: Use **Vibe CLI** and **Open Code** for all core development (generation, compilation, testing).
2. **Hermes Agent**: Use Hermes Agent for high-level project orchestration, planning, architecture design and only minor edits. Non-minor code edits are to be delegated to coding agents named in Rule #1 whenever possible.
3. **Avoid**:
   - Using Mistral models in OpenCode/Hermes (flaky behavior).
   - Generating assembly or low-level code in Web Vibe (no execution).
   - Copying foreign kernel source code verbatim (the project's anti-plagiarism agent and other guardrails will check for it)
   - Re-using any GPL-licensed source code

---
## **🤖 Agent-Specific Guidelines**
### **Vibe CLI (Local)**
| Use Case | Command | Notes |
|----------|---------|-------|
| **Generate code** | `vibe --model mistral-medium-3.5` | Use for new files (e.g., `pmm.c`). |
| **Edit code** | Paste file + prompt | Ask for refinements (e.g., "Optimize this for UNIXoid"). |
| **Debug** | Paste error + code | Ask for root cause (e.g., "Why does this triple-fault?"). |
| **Teleport** | `/teleport` | Sync to cloud for backup/collaboration. |
--
