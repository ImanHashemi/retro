---
name: iterative-design-review-before-implementation
description: Use when presenting implementation plans, architectural designs, multi-phase proposals, PLAN.md documents, or complex specifications that require user validation. Use when user requests design review, when proposing significant architectural changes, or when presenting technical proposals for new features.
---

For complex designs and implementation plans, follow an iterative review cycle:

1. **Initial Presentation**: Present the complete design/plan with clear structure and reasoning
2. **Wait for Feedback**: Do not proceed to implementation. Wait for user review and specific feedback
3. **Incorporate Changes**: Address all feedback points explicitly in the revised version
4. **Re-present for Review**: Show the updated design/plan highlighting what changed
5. **Repeat**: Continue cycles 2-4 until user gives explicit approval
6. **Implementation Gate**: Only proceed to coding after receiving clear approval (e.g., "looks good", "approved", "go ahead")

Signs you need this workflow:
- Creating or updating PLAN.md files
- Proposing new feature architectures
- Designing multi-phase implementations
- User asks "can you review this" or "thoughts on this approach"

Never skip directly from presenting a complex design to writing implementation code.