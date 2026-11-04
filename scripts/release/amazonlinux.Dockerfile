# Mitts Build — portable GNU release build environment
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/mitts
# SPDX-License-Identifier: MIT OR Apache-2.0
FROM public.ecr.aws/amazonlinux/amazonlinux:2023
RUN dnf install -y --allowerasing bash binutils coreutils gawk gcc git grep gzip python3 sed tar && dnf clean all
