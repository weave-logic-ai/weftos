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

our @PREFIX                     = ( '/home/aepod/dev/clawft/.claude/worktrees/m3-voice/target-m3voice/debug/build/openssl-sys-0e7b26b695d77320/out/openssl-build/build/src' );
our @libdir                     = ( '' );
our @BINDIR                     = ( '/home/aepod/dev/clawft/.claude/worktrees/m3-voice/target-m3voice/debug/build/openssl-sys-0e7b26b695d77320/out/openssl-build/build/src/apps' );
our @BINDIR_REL_PREFIX          = ( 'apps' );
our @LIBDIR                     = ( '/home/aepod/dev/clawft/.claude/worktrees/m3-voice/target-m3voice/debug/build/openssl-sys-0e7b26b695d77320/out/openssl-build/build/src' );
our @LIBDIR_REL_PREFIX          = ( '' );
our @INCLUDEDIR                 = ( '/home/aepod/dev/clawft/.claude/worktrees/m3-voice/target-m3voice/debug/build/openssl-sys-0e7b26b695d77320/out/openssl-build/build/src/include', '/home/aepod/dev/clawft/.claude/worktrees/m3-voice/target-m3voice/debug/build/openssl-sys-0e7b26b695d77320/out/openssl-build/build/src/include' );
our @INCLUDEDIR_REL_PREFIX      = ( 'include', './include' );
our @APPLINKDIR                 = ( '/home/aepod/dev/clawft/.claude/worktrees/m3-voice/target-m3voice/debug/build/openssl-sys-0e7b26b695d77320/out/openssl-build/build/src/ms' );
our @APPLINKDIR_REL_PREFIX      = ( 'ms' );
our @ENGINESDIR                 = ( '/home/aepod/dev/clawft/.claude/worktrees/m3-voice/target-m3voice/debug/build/openssl-sys-0e7b26b695d77320/out/openssl-build/build/src/engines' );
our @ENGINESDIR_REL_LIBDIR      = ( 'engines' );
our @MODULESDIR                 = ( '/home/aepod/dev/clawft/.claude/worktrees/m3-voice/target-m3voice/debug/build/openssl-sys-0e7b26b695d77320/out/openssl-build/build/src/providers' );
our @MODULESDIR_REL_LIBDIR      = ( 'providers' );
our @PKGCONFIGDIR               = ( '/home/aepod/dev/clawft/.claude/worktrees/m3-voice/target-m3voice/debug/build/openssl-sys-0e7b26b695d77320/out/openssl-build/build/src' );
our @PKGCONFIGDIR_REL_LIBDIR    = ( '.' );
our @CMAKECONFIGDIR             = ( '/home/aepod/dev/clawft/.claude/worktrees/m3-voice/target-m3voice/debug/build/openssl-sys-0e7b26b695d77320/out/openssl-build/build/src' );
our @CMAKECONFIGDIR_REL_LIBDIR  = ( '.' );
our $VERSION                    = '3.5.5';
our @LDLIBS                     =
    # Unix and Windows use space separation, VMS uses comma separation
    $^O eq 'VMS'
    ? split(/ *, */, '-ldl -pthread ')
    : split(/ +/, '-ldl -pthread ');

1;
