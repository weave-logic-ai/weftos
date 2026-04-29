package OpenSSL::safe::installdata;

use strict;
use warnings;
use Exporter;
our @ISA = qw(Exporter);
our @EXPORT = qw(
    @PREFIX
    @libdir
    @BINDIR @BINDIR_REL_PREFIX
    @LIBDIR @LIBDIR_REL_PREFIX
    @INCLUDEDIR @INCLUDEDIR_REL_PREFIX
    @APPLINKDIR @APPLINKDIR_REL_PREFIX
    @ENGINESDIR @ENGINESDIR_REL_LIBDIR
    @MODULESDIR @MODULESDIR_REL_LIBDIR
    @PKGCONFIGDIR @PKGCONFIGDIR_REL_LIBDIR
    @CMAKECONFIGDIR @CMAKECONFIGDIR_REL_LIBDIR
    $VERSION @LDLIBS
);

our @PREFIX                     = ( '/home/aepod/dev/clawft/.claude/worktrees/m3-voice/target-m3voice/debug/build/openssl-sys-0e7b26b695d77320/out/openssl-build/install' );
our @libdir                     = ( '/home/aepod/dev/clawft/.claude/worktrees/m3-voice/target-m3voice/debug/build/openssl-sys-0e7b26b695d77320/out/openssl-build/install/lib' );
our @BINDIR                     = ( '/home/aepod/dev/clawft/.claude/worktrees/m3-voice/target-m3voice/debug/build/openssl-sys-0e7b26b695d77320/out/openssl-build/install/bin' );
our @BINDIR_REL_PREFIX          = ( 'bin' );
our @LIBDIR                     = ( '/home/aepod/dev/clawft/.claude/worktrees/m3-voice/target-m3voice/debug/build/openssl-sys-0e7b26b695d77320/out/openssl-build/install/lib' );
our @LIBDIR_REL_PREFIX          = ( 'lib' );
our @INCLUDEDIR                 = ( '/home/aepod/dev/clawft/.claude/worktrees/m3-voice/target-m3voice/debug/build/openssl-sys-0e7b26b695d77320/out/openssl-build/install/include' );
our @INCLUDEDIR_REL_PREFIX      = ( 'include' );
our @APPLINKDIR                 = ( '/home/aepod/dev/clawft/.claude/worktrees/m3-voice/target-m3voice/debug/build/openssl-sys-0e7b26b695d77320/out/openssl-build/install/include/openssl' );
our @APPLINKDIR_REL_PREFIX      = ( 'include/openssl' );
our @ENGINESDIR                 = ( '/home/aepod/dev/clawft/.claude/worktrees/m3-voice/./target-m3voice/debug/build/openssl-sys-0e7b26b695d77320/out/openssl-build/install/lib/engines-3' );
our @ENGINESDIR_REL_LIBDIR      = ( 'engines-3' );
our @MODULESDIR                 = ( '/home/aepod/dev/clawft/.claude/worktrees/m3-voice/./target-m3voice/debug/build/openssl-sys-0e7b26b695d77320/out/openssl-build/install/lib/ossl-modules' );
our @MODULESDIR_REL_LIBDIR      = ( 'ossl-modules' );
our @PKGCONFIGDIR               = ( '/home/aepod/dev/clawft/.claude/worktrees/m3-voice/./target-m3voice/debug/build/openssl-sys-0e7b26b695d77320/out/openssl-build/install/lib/pkgconfig' );
our @PKGCONFIGDIR_REL_LIBDIR    = ( 'pkgconfig' );
our @CMAKECONFIGDIR             = ( '/home/aepod/dev/clawft/.claude/worktrees/m3-voice/./target-m3voice/debug/build/openssl-sys-0e7b26b695d77320/out/openssl-build/install/lib/cmake/OpenSSL' );
our @CMAKECONFIGDIR_REL_LIBDIR  = ( 'cmake/OpenSSL' );
our $VERSION                    = '3.5.5';
our @LDLIBS                     =
    # Unix and Windows use space separation, VMS uses comma separation
    $^O eq 'VMS'
    ? split(/ *, */, '-ldl -pthread ')
    : split(/ +/, '-ldl -pthread ');

1;
