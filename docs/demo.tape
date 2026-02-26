# VHS tape script for retro hero GIF
# Install: https://github.com/charmbracelet/vhs
# Run: vhs docs/demo.tape

Output docs/demo.gif

Set Shell "bash"
Set FontSize 15
Set Width 1000
Set Height 600
Set Theme "Catppuccin Mocha"
Set Padding 20
Set TypingSpeed 60ms

# Set up alias so "retro" calls our scripted output (hidden from recording)
Hide
Type "alias retro='./docs/demo-output.sh'"
Enter
Sleep 500ms
Type "clear"
Enter
Sleep 500ms
Show

# Scene 1: Analyze
Sleep 500ms
Type "retro analyze"
Enter
Sleep 5s

# Scene 2: Patterns
Type "retro patterns"
Enter
Sleep 8s

# Scene 3: Review
Type "retro review"
Enter
Sleep 2s
Type "1a 2a"
Enter
Sleep 4s
