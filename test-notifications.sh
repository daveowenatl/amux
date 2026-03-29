#!/bin/bash
# End-to-end notification system test script
# Run this INSIDE an amux terminal session (so AMUX_SOCKET_PATH is set)

AMUX="./target/release/amux"

echo "=== amux Notification System E2E Tests ==="
echo ""
echo "Prerequisites:"
echo "  1. amux is running (./target/release/amux-app)"
echo "  2. You have at least 2 workspaces open"
echo "  3. This script is running inside an amux terminal"
echo ""

if [ -z "$AMUX_SOCKET_PATH" ]; then
    echo "ERROR: AMUX_SOCKET_PATH not set. Run this inside amux."
    exit 1
fi

echo "Socket: $AMUX_SOCKET_PATH"
echo "Workspace: $AMUX_WORKSPACE_ID"
echo ""

# --- Test 1: Basic notification ---
echo "--- Test 1: Basic Notification ---"
echo "Sending a test notification..."
$AMUX notify "Hello from test script" --title "Test 1"
echo "  -> Check: notification ring appears on the pane, sidebar badge increments"
echo ""
read -p "Press Enter to continue..."

# --- Test 2: Workspace bubbling ---
echo ""
echo "--- Test 2: Workspace Bubbling ---"
echo "Switch to workspace 1 (top), then we'll send a notification to workspace 2."
read -p "Switch to workspace 1 and press Enter..."
echo "Sending notification to workspace 2..."
# We need workspace 2's ID — list workspaces first
echo "Current workspaces:"
$AMUX list
echo ""
echo "  -> Check: workspace that received the notification moved to top of sidebar"
echo ""
read -p "Press Enter to continue..."

# --- Test 3: System notification (unfocused) ---
echo ""
echo "--- Test 3: System Notification (Unfocused) ---"
echo "INSTRUCTIONS:"
echo "  1. Click away from amux (focus another app)"
echo "  2. Wait 3 seconds"
echo "  3. Watch for an OS notification toast"
echo ""
read -p "Click away from amux, then press Enter..."
sleep 3
$AMUX notify "This should appear as an OS toast!" --title "System Toast Test"
echo "  -> Check: macOS notification center shows 'System Toast Test'"
echo "  -> Check: dock badge shows '1' (or incremented count)"
echo ""
read -p "Press Enter to continue..."

# --- Test 4: Suppressed sound (focused, different pane) ---
echo ""
echo "--- Test 4: Suppressed Sound Feedback ---"
echo "INSTRUCTIONS:"
echo "  1. Make sure amux is focused"
echo "  2. If you have multiple panes, focus one"
echo "  3. We'll send a notification to a different pane"
echo ""
echo "  -> Listen for: a short 440Hz beep (system sound)"
echo ""
read -p "Focus amux and press Enter..."
$AMUX notify "Sound test notification" --title "Sound Test"
echo "  -> Check: you heard a beep (if app was focused and notification was for different pane)"
echo ""
read -p "Press Enter to continue..."

# --- Test 5: Focused pane (silent) ---
echo ""
echo "--- Test 5: Same-Pane Notification (Silent) ---"
echo "Sending notification to THIS pane (should be silent, flash only)..."
$AMUX notify "Same pane notification" --title "Silent Test"
echo "  -> Check: flash animation on this pane, but NO sound and NO OS toast"
echo "  -> Check: notification appears in notification panel but marked as read"
echo ""
read -p "Press Enter to continue..."

# --- Test 6: Dock badge ---
echo ""
echo "--- Test 6: Dock Badge ---"
echo "Checking dock badge..."
echo "  -> Check: macOS dock icon shows unread count badge"
echo "  -> Now clear notifications to verify badge clears:"
$AMUX clear-notifications
echo "  -> Check: dock badge disappeared"
echo ""
read -p "Press Enter to continue..."

# --- Test 7: Rapid fire (tests worker thread) ---
echo ""
echo "--- Test 7: Rapid Fire (Worker Thread Stress) ---"
echo "Sending 10 notifications rapidly..."
for i in $(seq 1 10); do
    $AMUX notify "Rapid notification #$i" --title "Burst Test" &
done
wait
echo "  -> Check: all 10 arrived, no crashes, no thread explosion"
echo "  -> Check: dock badge shows correct count"
echo ""
read -p "Press Enter to continue..."

echo ""
echo "=== All Tests Complete ==="
echo ""
echo "Config toggle tests (manual):"
echo "  Edit ~/.config/amux/config.toml and add:"
echo ""
echo '  [notifications]'
echo '  auto_reorder_workspaces = false'
echo '  system_notifications = false'
echo '  dock_badge = false'
echo ''
echo '  [notifications.sound]'
echo '  sound = "none"'
echo ""
echo "  Then restart amux and re-run tests to verify each toggle works."
