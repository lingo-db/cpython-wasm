export BUILD_ENV_SCRIPTS_DIR=
#export PATH=$(pwd)/custom_path/:$PATH
export CC=$(pwd)/cross-compile-links/cc
export CXX=$(pwd)/cross-compile-links/c++

NUMPY_DIR=$(pwd)/../numpy
SUPPORT_DIR=$(pwd)
BUILD_DIR=$(pwd)/build

export VERBOSE=1

python $NUMPY_DIR/vendored-meson/meson/meson.py setup $NUMPY_DIR $BUILD_DIR -Dbuildtype=release -Db_ndebug=if-release -Db_vscrt=md -Dallow-noblas=true --cross-file=$SUPPORT_DIR/wasi-sdk-config.cross --native-file=$SUPPORT_DIR/meson-python-native-file.ini
python $NUMPY_DIR compile -C $BUILD_DIR
python $NUMPY_DIR/vendored-meson/meson/meson.py install -C $BUILD_DIR --only-changed --destdir dist
