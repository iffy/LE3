# Ready to do

# Needs description

[ ] Snap to offset places (dashed lines)
[ ] Let me right click a Sketch to export to DXF. In the context pane, show options like "include construction lines".
[ ] STL export
[ ] Step export
[ ] Technical drawings
[ ] Click lines multiple times to draw polygon (after snapping). Let me choose relative angles from last line (or from horizontal/vertical)
[ ] Click drag lines to make curves

# Done

[X] When focus is on a variable in the variable pane (either input), highlight all the elements in the element pane that make use of that variable.
[X] Make the take screenshot function take a screenshot of just the 3D viewing space by default (without the bear HUD, if possible). A parameter to the function can be passed to take a screenshot of the whole window, too. If not filename is given, save the screenshot as `screenshot-bearcad.png`
[X] Add basic snapping. When drawing things or moving them, snap to nearby things (i.e. vertex to line, vertex to vertex, etc...). If the user decides to leave something at the snap point, add an appropriate constraint (e.g. coincident). One of the snaps to support is snapping to the midpoint of a line. Add a toggle to the context menu to enable/disable snapping and only show it when on a tool that uses it.
[X] Extrude. Add an extrude tool (use the extrude icon). Click coplanar faces to toggle inclusion (hover-highlighted); a normal gizmo + distance input (expressions) set the depth (+/-) with a live preview; commit creates a 3D solid (Extrusion + dependent Body elements); double-click / right-click > Edit re-opens it; extrude-to-object constrains the depth to a snapped vertex/face/plane's extended plane.
