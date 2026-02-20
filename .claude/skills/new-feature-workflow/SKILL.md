---
name: new-feature-workflow
description: Use when starting work on new features, when the user mentions creating a feature, or when asked to build something new. Keywords: feature, implement, add functionality, new capability, build.
---

For all new features, follow this four-stage workflow:

## Stage 1: Brainstorming
Use the brainstorming skill (`/brainstorming`) to explore requirements and design with the user.

## Stage 2: Design Document
After brainstorming approval, write a design document:
1. Create doc in `docs/` directory
2. Include architecture, API design, data models, and trade-offs
3. Get user approval before proceeding

## Stage 3: Implementation Plan
Create a detailed implementation plan:
1. Write plan file (typically `PLAN.md` or in `docs/`)
2. Break down into concrete tasks with file changes
3. Commit the plan to the repository

## Stage 4: Implementation & PR
Execute the implementation:
1. Create feature branch
2. Implement changes according to plan
3. Merge design doc and plan into feature branch
4. Create PR with both documents included

Do not skip stages or combine them without explicit user approval.