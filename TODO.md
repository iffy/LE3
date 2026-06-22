# Ready to do

[ ] Extrude. Add an extrude tool (use the extrude icon). Once the tool is selected, you can click faces within the same plane to toggle whether they are included in the extrusion or not. Highlight selectable faces on hover. When one or more faces are selected, a single gizmo should appear with an arrow pointing normal to the plane. You can drag the gizmo handle to extrude the face to that point. You should be able to extrude in either the positive or negative direction. There should also be a text input you can use to set the extrusion distance (using expressions and variables). If only the gizmo is used, there are no constraints. An extrusion will create a 3D solid. In the elements pane, there will be an "Extrusion" element that represents the action of extruding. Double-clicking it (or right-click > Edit) lets you edit the extrusion. While dditing an extrusion, you can toggle which faces are part of the extrusion by clicking on them and you can adjust the extrusion length using the gizmo or text input. Also make it so that I can extrude to an object (vertex, face, plane). The UI for this is, once I click the gizmo to start dragging, I can drag to some object (with snapping hover). If I let go, constrain the extrusion to that vertex/face/plane. The extrusion should only happen normal to the faces being extruded. If I extrude to a plane/face, the extrusion should go to the extended version of that plane/face in space. 

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
[X] Make the take screenshot function take a screenshot of just the 3D viewing space by default (without the bear HUD, if possible). A parameter to the function can be passed to take a screenshot of the whole window, too. If not filename is given, save the screenshot as `screenshot-le3.png`
[X] Add basic snapping. When drawing things or moving them, snap to nearby things (i.e. vertex to line, vertex to vertex, etc...). If the user decides to leave something at the snap point, add an appropriate constraint (e.g. coincident). One of the snaps to support is snapping to the midpoint of a line. Add a toggle to the context menu to enable/disable snapping and only show it when on a tool that uses it.
