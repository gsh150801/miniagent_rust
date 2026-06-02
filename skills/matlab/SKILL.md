---
name: matlab
description: MATLAB and GNU Octave numerical computing for matrix operations, data analysis, visualization, and scientific computing. Use when writing MATLAB/Octave scripts for linear algebra, signal processing, i
triggers:
  - matlab
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 5
---

# MATLAB/Octave Scientific Computing

MATLAB is a numerical computing environment optimized for matrix operations and scientific computing. GNU Octave is a free, open-source alternative with high MATLAB compatibility.

## Quick Start

**Running MATLAB scripts:**
```bash
# MATLAB (commercial)
matlab -nodisplay -nosplash -r "run('script.m'); exit;"

# GNU Octave (free, open-source)
octave script.m
```

**Install GNU Octave:**
```bash
# macOS
brew install octave

# Ubuntu/Debian
sudo apt install octave

# Windows - download from https://octave.org/download
```

## Core Capabilities

### 1. Matrix Operations

MATLAB operates fundamentally on matrices and arrays:

```matlab
% Create matrices
A = [1 2 3; 4 5 6; 7 8 9];  % 3x3 matrix
v = 1:10;                     % Row vector 1 to 10
v = linspace(0, 1, 100);      % 100 points from 0 to 1

% Special matrices
I = eye(3);          % Identity matrix
Z = zeros(3, 4);     % 3x4 zero matrix
O = ones(2, 3);      % 2x3 ones matrix
R = rand(3, 3);      % Random uniform
N = randn(3, 3);     % Random normal

% Matrix operations
B = A';              % Transpose
C = A * B;           % Matrix multiplication
D = A .* B;          % Element-wise multiplication
E = A \ b;           % Solve linear system Ax = b
F = inv(A);          % Matrix inverse
```

For complete matrix operations, see [references/matrices-arrays.md](references/matrices-arrays.md).

### 2. Linear Algebra

```matlab
% Eigenvalues and eigenvectors
[V, D] = eig(A);     % V: eigenvectors, D: diagonal eigenvalues

% Singular value decomposition
[U, S, V] = svd(A);

% Matrix decompositions
[L, U] = lu(A);      % LU decomposition
[Q, R] = qr(A);      % QR decomposition
R = chol(A);         % Cholesky (symmetric positive definite)

% Solve linear systems
x = A \ b;           % Preferred method
x = linsolve(A, b);  % With options
x = inv(A) * b;      % Less efficient
```

For comprehensive linear algebra, see [references/mathematics.md](references/mathematics.md).

### 3. Plotting and Visualization

```matlab
% 2D Plots
x = 0:0.1:2*pi;
y = sin(x);
plot(x, y, 'b-', 'LineWidth', 2);
xlabel('x'); ylabel('sin(x)');
title('Sine Wave');
grid on;

% Multiple plots
hold on;
plot(x, cos(x), 'r--');
legend('sin', 'cos');
hold off;

% 3D Surface
[X, Y] = meshgrid(-2:0.1:2, -2:0.1:2);
Z = X.^2 + Y.^2;
surf(X, Y, Z);
colorbar;

% Save figures
saveas(gcf, 'plot.png');
print('-dpdf', 'plot.pdf');
```

For complete visualization guide, see [references/graphics-visualization.md](references/graphics-visualization.md).

### 4. Data Import/Export

```matlab
% Read tabular data
T = readtable('data.csv');
M = readmatrix('data.csv');

% Write data
writetable(T, 'output.csv');
writematrix(M, 'output.csv');

% MAT files (MATLAB native)
save('data.mat', 'A', 'B', 'C');  % Save variables
load('data.mat');                   % Load all
S = load('data.mat', 'A');         % Load specific

% Images
img = imread('image.png');
imwrite(img, 'output.jpg');
```

For complete I/O guide, see [referen

... (truncated from original)