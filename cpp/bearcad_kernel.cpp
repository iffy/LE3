// OCCT-backed implementation of the BearCAD kernel C ABI (see bearcad_kernel.hpp).
//
// Only compiled when BearCAD is built with `--features occt`; the `cc` build in
// build.rs pulls this in and links it against the OCCT static libraries.

#include "bearcad_kernel.hpp"

#include <BRepPrimAPI_MakeBox.hxx>
#include <BRepPrimAPI_MakePrism.hxx>
#include <BRepBuilderAPI_MakePolygon.hxx>
#include <BRepBuilderAPI_MakeFace.hxx>
#include <BRepOffsetAPI_ThruSections.hxx>
#include <BRepAlgoAPI_Fuse.hxx>
#include <BRepAlgoAPI_Cut.hxx>
#include <BRepAlgoAPI_Common.hxx>
#include <BRepMesh_IncrementalMesh.hxx>
#include <BRep_Tool.hxx>
#include <BRepGProp.hxx>
#include <GProp_GProps.hxx>
#include <Poly_Triangulation.hxx>
#include <TopExp_Explorer.hxx>
#include <TopoDS.hxx>
#include <TopoDS_Face.hxx>
#include <TopoDS_Shape.hxx>
#include <TopoDS_Solid.hxx>
#include <TopLoc_Location.hxx>
#include <TopAbs_Orientation.hxx>
#include <gp_Pnt.hxx>
#include <gp_Vec.hxx>
#include <Standard_Failure.hxx>
#include <Standard_Version.hxx>

#include <vector>

// Opaque owned BREP shape handle exposed across the C ABI.
struct BearcadShape {
    TopoDS_Shape shape;
};

extern "C" double bearcad_kernel_box_volume(double dx, double dy, double dz) {
    try {
        BRepPrimAPI_MakeBox mk(dx, dy, dz);
        TopoDS_Solid solid = mk.Solid();
        GProp_GProps props;
        BRepGProp::VolumeProperties(solid, props);
        return props.Mass();
    } catch (const Standard_Failure&) {
        // Surface OCCT failures as a sentinel the Rust side treats as "kernel error"
        // rather than letting a C++ exception unwind across the FFI boundary (UB).
        return -1.0;
    } catch (...) {
        return -1.0;
    }
}

extern "C" const char* bearcad_kernel_occt_version(void) {
    return OCC_VERSION_STRING_EXT;
}

extern "C" BearcadShape* bearcad_shape_prism(const double* xyz, unsigned long n_pts,
                                             double dx, double dy, double dz) {
    if (xyz == nullptr || n_pts < 3) {
        return nullptr;
    }
    try {
        BRepBuilderAPI_MakePolygon poly;
        for (unsigned long i = 0; i < n_pts; ++i) {
            poly.Add(gp_Pnt(xyz[3 * i], xyz[3 * i + 1], xyz[3 * i + 2]));
        }
        poly.Close();
        if (!poly.IsDone()) {
            return nullptr;
        }
        BRepBuilderAPI_MakeFace face(poly.Wire());
        if (!face.IsDone()) {
            return nullptr;
        }
        BRepPrimAPI_MakePrism prism(face.Face(), gp_Vec(dx, dy, dz));
        return new BearcadShape{prism.Shape()};
    } catch (const Standard_Failure&) {
        return nullptr;
    } catch (...) {
        return nullptr;
    }
}

extern "C" BearcadShape* bearcad_shape_loft(const double* bottom_xyz, const double* top_xyz,
                                            unsigned long n_pts) {
    if (bottom_xyz == nullptr || top_xyz == nullptr || n_pts < 3) {
        return nullptr;
    }
    try {
        BRepBuilderAPI_MakePolygon bottom;
        BRepBuilderAPI_MakePolygon top;
        for (unsigned long i = 0; i < n_pts; ++i) {
            bottom.Add(gp_Pnt(bottom_xyz[3 * i], bottom_xyz[3 * i + 1], bottom_xyz[3 * i + 2]));
            top.Add(gp_Pnt(top_xyz[3 * i], top_xyz[3 * i + 1], top_xyz[3 * i + 2]));
        }
        bottom.Close();
        top.Close();
        if (!bottom.IsDone() || !top.IsDone()) {
            return nullptr;
        }
        // isSolid = true (cap the ends), ruled = true (planar strips between
        // corresponding edges rather than a smooth interpolation).
        BRepOffsetAPI_ThruSections gen(Standard_True, Standard_True);
        gen.AddWire(bottom.Wire());
        gen.AddWire(top.Wire());
        gen.Build();
        if (!gen.IsDone()) {
            return nullptr;
        }
        return new BearcadShape{gen.Shape()};
    } catch (const Standard_Failure&) {
        return nullptr;
    } catch (...) {
        return nullptr;
    }
}

extern "C" BearcadShape* bearcad_shape_boolean(const BearcadShape* a, const BearcadShape* b,
                                               int op) {
    if (a == nullptr || b == nullptr) {
        return nullptr;
    }
    try {
        TopoDS_Shape result;
        switch (op) {
            case 0: result = BRepAlgoAPI_Fuse(a->shape, b->shape).Shape(); break;
            case 1: result = BRepAlgoAPI_Cut(a->shape, b->shape).Shape(); break;
            case 2: result = BRepAlgoAPI_Common(a->shape, b->shape).Shape(); break;
            default: return nullptr;
        }
        return new BearcadShape{result};
    } catch (const Standard_Failure&) {
        return nullptr;
    } catch (...) {
        return nullptr;
    }
}

extern "C" double bearcad_shape_volume(const BearcadShape* shape) {
    if (shape == nullptr) {
        return -1.0;
    }
    try {
        GProp_GProps props;
        BRepGProp::VolumeProperties(shape->shape, props);
        return props.Mass();
    } catch (const Standard_Failure&) {
        return -1.0;
    } catch (...) {
        return -1.0;
    }
}

extern "C" double* bearcad_shape_tessellate(const BearcadShape* shape, double deflection,
                                            unsigned long* out_tri_count) {
    if (out_tri_count != nullptr) {
        *out_tri_count = 0;
    }
    if (shape == nullptr || out_tri_count == nullptr) {
        return nullptr;
    }
    try {
        // Mutating meshing is stored on the shape's TShape; work on a copy of the
        // handle (cheap, shares the underlying TShape) so the const contract holds
        // at the Rust boundary while OCCT attaches its triangulation.
        TopoDS_Shape s = shape->shape;
        BRepMesh_IncrementalMesh mesher(s, deflection, Standard_False, 0.5, Standard_True);
        mesher.Perform();

        std::vector<double> tris;
        for (TopExp_Explorer ex(s, TopAbs_FACE); ex.More(); ex.Next()) {
            const TopoDS_Face& face = TopoDS::Face(ex.Current());
            TopLoc_Location loc;
            Handle(Poly_Triangulation) tri = BRep_Tool::Triangulation(face, loc);
            if (tri.IsNull()) {
                continue;
            }
            const gp_Trsf& trsf = loc.Transformation();
            const bool reversed = face.Orientation() == TopAbs_REVERSED;
            for (Standard_Integer t = 1; t <= tri->NbTriangles(); ++t) {
                Standard_Integer n1, n2, n3;
                tri->Triangle(t).Get(n1, n2, n3);
                if (reversed) {
                    std::swap(n2, n3);
                }
                const Standard_Integer idx[3] = {n1, n2, n3};
                for (int k = 0; k < 3; ++k) {
                    gp_Pnt p = tri->Node(idx[k]).Transformed(trsf);
                    tris.push_back(p.X());
                    tris.push_back(p.Y());
                    tris.push_back(p.Z());
                }
            }
        }
        if (tris.empty()) {
            return nullptr;
        }
        *out_tri_count = static_cast<unsigned long>(tris.size() / 9);
        double* out = new double[tris.size()];
        std::copy(tris.begin(), tris.end(), out);
        return out;
    } catch (const Standard_Failure&) {
        return nullptr;
    } catch (...) {
        return nullptr;
    }
}

extern "C" void bearcad_tri_free(double* tris) {
    delete[] tris;
}

extern "C" void bearcad_shape_free(BearcadShape* shape) {
    delete shape;
}
