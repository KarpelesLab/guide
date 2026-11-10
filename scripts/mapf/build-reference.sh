#!/bin/sh
# Builds the reference MAPF solvers from source into DIR: CBSH2-RTC (optimal) and EECBS
# (bounded-suboptimal), by Jiaoyang Li et al., under USC's licence for research and
# non-profit use, and LaCAM3, by Keisuke Okumura, under the MIT licence. Needs git, cmake, a
# C++17 compiler and Boost (program_options, filesystem). None of them is copied into guide.
DIR=${1:?usage: build-reference.sh DIR}
mkdir -p "$DIR" && cd "$DIR" || exit 1
fetch() {
  [ -d "$1" ] || git clone -q --recursive "https://github.com/$2/$1.git" || exit 1
  (cd "$1" && git checkout -q "$3" && git submodule -q update --init --recursive) || exit 1
}
fetch CBSH2-RTC Jiaoyang-Li a834df1e16c1668a677b68fef6699371c4233d33
fetch EECBS Jiaoyang-Li 06ec70585dc4ad73177580a4ef91138335fd4d24
fetch lacam3 Kei18 1c3ecf8ade40298f16047544634e9c0c1cf3b93c
# Boost.System has been header-only since 1.69, and newer Boost has no library to find.
for d in CBSH2-RTC EECBS; do
  sed -i 's/program_options system filesystem/program_options filesystem/' "$d/CMakeLists.txt" || exit 1
  (cd "$d" && cmake -B build -DCMAKE_BUILD_TYPE=RELEASE -DCMAKE_POLICY_VERSION_MINIMUM=3.5 >/dev/null &&
    make -C build -j"$(nproc)" >/dev/null) || exit 1
done
(cd lacam3 && cmake -B build -DCMAKE_BUILD_TYPE=Release >/dev/null &&
  make -C build -j"$(nproc)" main >/dev/null) || exit 1
