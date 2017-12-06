
import numpy as np

import matplotlib.pyplot as plt


# Thermristor B-equation (T in Kelvin)
# R = R_N * exp(-B ( 1/T_N - 1/T ))
# T = B / ln (R / r_inf)
# with r_inf = R_N * exp(-B / T_N)
# T_N = 25 deg Celsius
# R_N = 100k Ohm

# Steinhard-Hart equation
# 1/T = a + b ln(R) + c (ln(R))**3
# or
# R = exp( 3root( y - x/2 ) - 3root( y + x/2 ))
# x = 1/c ( a - 1/T)
# y = sqrt( (b/(3*c))**3 + (x/2)**2 )

# equals B-equation when
# a=(1/T_0)-(1/B)*ln(R_0)
# b=1/B
# c=0

# LSQ     Ah=y    h=(A^T A)^-1 A^T y

# Solve Steinhard-Hart with LSQ using this gls

# [1/T_i]       [ 1   ln(R_i) ln**3(R_i) ]  [ a ]
# [  .  ]       [ .      .          .    ]  [ b ]
# [  .  ]   =   [ .      .          .    ]  [ c ]
# [  .  ]       [ .      .          .    ]



# raw -> R
# raw = R/(R+100k) * 1023
# <=>  raw*R + raw*100k = R * 1023
# <=>  raw * 100k = R ( 1023 - raw )
# <=>  R = (raw * 100k) / ( 1023 - raw )
def raw2R(raw):
    return (raw * 100000) / (1023 - raw)

# Solve for one temperature sensor
# Input: Measurements     [t]     [d]
#                         [.]     [ ]
#                         [.]     [ ]
def lsq(temperatures, raws):
    assert len(temperatures) == len(raws)
    R = np.array([raw2R(r) for r in raws])
    Tinv = np.array([1/(t+273.15) for t in temperatures])

    A = np.zeros((len(R), 3))
    for i, r in enumerate(R):
        A[i, 0] = 1
        A[i, 1] = np.log(r)
        A[i, 2] = A[i, 1] ** 3

    h = np.linalg.lstsq(A, Tinv)
    return h[0]
    pass



def plot(a, b, c):
    ts = np.arange(-20., 30., 1.)
    ts = ts + 273.15
    # x = 1/c ( a - 1/T)
    x = 1./c * ( a - 1./ts)
    # y = sqrt( (b/(3*c))**3 + (x/2)**2 )
    y = np.sqrt( (b/(3*c))**3 + (x/2)**2 )
    # R = exp( 3root( y - x/2 ) - 3root( y + x/2 ))
    R = np.exp( ( y - x/2 )**(1./3) -( y + x/2 )**(1./3) )


    plt.plot(x, y, 'r')

def plot2(a, b, c):
    raws = np.arange(100., 900., 1.)
    Rs = raw2R(raws)

    Tinv = a + b * np.log(Rs) + c * (np.log(Rs))**3
    Ts = (1/Tinv) - 273.15
    plt.plot(raws, Ts, 'r')



    #
# -1.6  ,  720.8   720.6   633.6   590.2   624.7   691.0
# 16.2  ,  590.2   595.6   539.1   513.8   537.5   585.0
# -18.5 ,  811.2   806.7   698.4   650.6   692.9   771.1
# 2.4   ,  700.0   701.1   620.1   580.9   614.5   678.8

temps = np.array([-1.6, 16.2, -18.5, 2.4])
sensors_raws = [ np.array([720.8 ,590.2 ,811.2 ,700.0]),
                 np.array([720.6, 595.6, 806.7, 701.1]),
                 np.array([633.6, 539.1, 698.4, 620.1]),
                 np.array([590.2, 513.8, 650.6, 580.9]),
                 np.array([624.7, 537.5, 692.9, 614.5]),
                 np.array([691.0, 585.0, 771.1, 678.8]) ]



coefficients = [lsq(temps, sr) for sr in sensors_raws]

print coefficients

for cs, sr in zip(coefficients, sensors_raws):
    plot2(cs[0], cs[1], cs[2])
    plt.plot(sr, temps, 'o', label='Original data', markersize=10)

plt.show()

