#!/usr/bin/env bash
# Scripted retro output for demo GIF recording.
# Usage: ./docs/demo-output.sh <analyze|patterns|review>

RESET='\033[0m'
BOLD='\033[1m'
DIM='\033[2m'
CYAN='\033[36m'
GREEN='\033[32m'
YELLOW='\033[33m'
WHITE='\033[37m'
BOLD_GREEN='\033[1;32m'
BOLD_YELLOW='\033[1;33m'
BOLD_WHITE='\033[1;37m'

case "$1" in
  analyze)
    echo -e "${CYAN}Step 1/3: Ingesting new sessions...${RESET}"
    sleep 0.3
    echo -e "  ${GREEN}4${RESET} new sessions ingested"
    sleep 0.5
    echo -e "${CYAN}Step 2/3: Analyzing sessions (window: 14d)...${RESET}"
    echo -e "  ${DIM}This may take a minute (AI-powered analysis)...${RESET}"
    sleep 2
    echo -e "${CYAN}Step 3/3: Recording audit log...${RESET}"
    sleep 0.3
    echo ""
    echo -e "  Batch 1/1: 12 sessions, 48K chars → 892 tokens out, ${GREEN}3${RESET} new + ${YELLOW}1${RESET} updated"
    echo -e "    ${DIM}Found recurring testing workflow and two explicit directives about package management and commit style.${RESET}"
    echo ""
    echo -e "${BOLD_GREEN}Analysis complete!${RESET}"
    echo -e "  ${WHITE}Sessions analyzed:${RESET} ${CYAN}12${RESET}"
    echo -e "  ${WHITE}New patterns:${RESET}      ${GREEN}3${RESET}"
    echo -e "  ${WHITE}Updated patterns:${RESET}  ${YELLOW}1${RESET}"
    echo -e "  ${WHITE}Total patterns:${RESET}    ${CYAN}4${RESET}"
    echo -e "  ${WHITE}Tokens:${RESET}            ${CYAN}52340${RESET} in / ${CYAN}892${RESET} out"
    echo ""
    echo -e "Run ${CYAN}retro patterns${RESET} to see discovered patterns."
    ;;

  patterns)
    echo -e "Patterns (${GREEN}3 discovered${RESET}, ${CYAN}1 active${RESET}, 0 archived)"
    echo ""
    echo -e "  ${YELLOW}[discovered]${RESET} repetitive_instruction (confidence: ${BOLD}82%${RESET}, seen: 4x)"
    echo -e "    \"User consistently tells the agent to use uv instead of pip for all Python package operations\""
    echo -e "    → ${CYAN}claude_md${RESET}"
    echo ""
    echo -e "  ${YELLOW}[discovered]${RESET} workflow_pattern (confidence: ${BOLD}75%${RESET}, seen: 3x)"
    echo -e "    \"User guides agent through run-tests-then-lint-then-commit workflow before every PR\""
    echo -e "    → ${CYAN}skill${RESET}"
    echo ""
    echo -e "  ${YELLOW}[discovered]${RESET} repetitive_instruction (confidence: ${BOLD}78%${RESET}, seen: 1x)"
    echo -e "    \"Always use conventional commit messages with type prefix (feat:, fix:, docs:)\""
    echo -e "    → ${CYAN}claude_md${RESET}"
    echo ""
    echo -e "  ${CYAN}[active]${RESET} recurring_mistake (confidence: ${BOLD}88%${RESET}, seen: 5x)"
    echo -e "    \"Agent forgets to run database migrations before running integration tests\""
    echo -e "    → ${CYAN}claude_md${RESET}"
    ;;

  review)
    echo -e "${BOLD_WHITE}Pending review (2 items):${RESET}"
    echo ""
    echo -e "  ${CYAN}1.${RESET} [claude_md] Always use uv instead of pip for Python package management"
    echo -e "  ${CYAN}2.${RESET} [skill] Pre-PR checklist: run tests, lint, then commit"
    echo ""
    echo -n -e "Enter actions (e.g. ${DIM}1a 2s 3d${RESET}, ${DIM}all:a${RESET}): "
    read -r _input
    echo ""
    echo -e "  ${GREEN}Applied:${RESET} #1 claude_md rule added to CLAUDE.md"
    echo -e "  ${GREEN}Applied:${RESET} #2 skill written to .claude/skills/pre-pr-checklist/SKILL.md"
    echo ""
    echo -e "${BOLD_GREEN}2 items applied.${RESET} Shared changes committed to ${CYAN}retro/updates-20260226-091500${RESET}."
    echo -e "Run ${CYAN}retro sync${RESET} after PR is merged."
    ;;

  *)
    echo "Usage: $0 <analyze|patterns|review>"
    exit 1
    ;;
esac
