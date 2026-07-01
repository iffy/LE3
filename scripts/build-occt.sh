#!/usr/bin/env bash
# Build the OpenCASCADE (OCCT) geometry kernel as static libraries for BearCAD (#86).
#
# BearCAD links OCCT statically (LGPL 2.1 permits this provided we ship the means
# to relink against a different OCCT — that's what OCCT_DIR + this script provide;
# see README.md "Building with the OCCT kernel"). OCCT source comes from the
# `third_party/OCCT` git submodule.
#
# Usage:
#   scripts/build-occt.sh              # build into third_party/OCCT/occt-install
#   cargo build --features occt        # then build BearCAD against it
#
# To build BearCAD against your *own* OCCT instead of this script's output, set
# OCCT_DIR to an install prefix containing include/opencascade and lib/libTK*.a
# and skip this script entirely.
#
# Only the modeling toolkits are built (FoundationClasses, ModelingData,
# ModelingAlgorithms). Visualization, DataExchange, ApplicationFramework, Draw and
# the FreeType/TCL/TK/VTK dependencies are all disabled — they aren't needed for
# the current kernel surface (solids, booleans, mass properties).

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
occt_src="$repo_root/third_party/OCCT"
occt_build="$occt_src/occt-build"
occt_install="$occt_src/occt-install"

if [ ! -f "$occt_src/CMakeLists.txt" ]; then
  echo "error: OCCT submodule missing at $occt_src" >&2
  echo "       run: git submodule update --init --depth 1 third_party/OCCT" >&2
  exit 1
fi

command -v cmake >/dev/null 2>&1 || { echo "error: cmake not found on PATH" >&2; exit 1; }

jobs="$( (getconf _NPROCESSORS_ONLN 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo 4) )"

echo ">> Configuring OCCT (static, modeling-only) ..."
# -ffunction-sections/-fdata-sections put each function/data item in its own
# section so BearCAD's link-time dead-strip (build.rs: -dead_strip / --gc-sections)
# can drop every OCCT function the final binary never calls — only used code paths
# end up in the shipped executable.
cmake -S "$occt_src" -B "$occt_build" \
  -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_INSTALL_PREFIX="$occt_install" \
  -DCMAKE_CXX_FLAGS="-ffunction-sections -fdata-sections" \
  -DCMAKE_C_FLAGS="-ffunction-sections -fdata-sections" \
  -DBUILD_LIBRARY_TYPE=Static \
  -DBUILD_MODULE_FoundationClasses=ON \
  -DBUILD_MODULE_ModelingData=ON \
  -DBUILD_MODULE_ModelingAlgorithms=ON \
  -DBUILD_MODULE_Visualization=OFF \
  -DBUILD_MODULE_ApplicationFramework=OFF \
  -DBUILD_MODULE_DataExchange=OFF \
  -DBUILD_MODULE_Draw=OFF \
  -DBUILD_MODULE_DETools=OFF \
  -DUSE_FREETYPE=OFF \
  -DUSE_TK=OFF \
  -DUSE_TCL=OFF \
  -DUSE_VTK=OFF \
  -DUSE_FREEIMAGE=OFF \
  -DUSE_RAPIDJSON=OFF \
  -DUSE_OPENGL=OFF \
  -DUSE_GLES2=OFF \
  -DBUILD_DOC_Overview=OFF

echo ">> Building OCCT with $jobs jobs (this takes a while) ..."
cmake --build "$occt_build" --config Release -j "$jobs"

echo ">> Installing OCCT into $occt_install ..."
cmake --install "$occt_build" --config Release

echo ">> Done. OCCT static libs are in $occt_install/lib"
echo ">> Now build BearCAD with: cargo build --features occt"
