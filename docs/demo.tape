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

# Title pause
Sleep 1s

# Show status — what retro knows about your sessions
Type "retro status"
Enter
Sleep 3s

# Show discovered patterns
Type "retro patterns"
Enter
Sleep 10s

